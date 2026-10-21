"""
The executor (RB-04 step 8, docs/architecture/pipeline.md#the-executor):
renders the surfaces `config.apply` assigned from the analysed audio and
publishes them. Each step works in dependency order: every analysed packet
goes to the beat signal and the activity, then each surface's stack is
rendered bottom first, then the frames and signals are published.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Protocol, cast

import numpy as np

from folo_source.core import Frame, Signal
from folo_source.effects.visualizers import EFFECT_ID
from folo_source.inputs.audio.analyzer import QUIET_DB, SAMPLERATE, Analyzer, Spectrum
from folo_source.inputs.audio.input import MAX_DT
from folo_source.publish.publisher import Publisher
from folo_source.signals.beat import BeatSignal
from folo_source.surfaces.renderer import SurfaceRenderer

# The signals this engine has
SIGNALS = ('beat', 'activity')

Layers = Sequence[Mapping[str, Any]]


class Media(Protocol):
    """What the executor reads of now playing (MediaInput)."""

    @property
    def colors(self) -> tuple[str, ...]: ...

    @property
    def track(self) -> int: ...


@dataclass(frozen=True, slots=True)
class Step:
    """Analysed audio for one step: its spectrum, sample frames and seconds."""

    spectrum: Spectrum
    frames: int
    dt: float

    @staticmethod
    def silence(dt: float) -> Step:
        """A step of silence lasting `dt`, for idle rendering."""
        return Step(Spectrum.silent(), int(dt * SAMPLERATE), dt)


@dataclass(slots=True)
class Activity:
    """
    How active the audio is (the activity notification): active while its
    loudest bin without gain reaches the silence threshold, and the seconds
    of silence so far.
    """

    active: bool = False
    level_db: float = -120.0
    quiet_s: float = 0.0

    def update(self, spectrum: Spectrum, dt: float) -> None:
        self.level_db = float(spectrum.loudest)
        self.active = self.level_db >= QUIET_DB
        self.quiet_s = 0.0 if self.active else self.quiet_s + dt

    def params(self) -> dict[str, Any]:
        return {
            'active': self.active,
            'level_db': round(self.level_db, 1),
            'quiet_s': round(self.quiet_s, 2),
        }

    def values(self) -> tuple[float, ...]:
        return (float(self.active), self.level_db, self.quiet_s)


@dataclass(slots=True)
class _Assigned:
    """A surface `config.apply` assigned, with its layers and renderer."""

    index: int
    surface: Mapping[str, Any]
    layers: Sequence[Mapping[str, Any]]
    beat_offset_ms: int
    album_colors: bool
    renderer: SurfaceRenderer


class Executor:
    """
    Renders and publishes the configured surfaces. `send` takes every frame
    to send (the link's send_frame). Calls come from one thread at a time;
    the caller serialises apply() and step().
    """

    def __init__(
        self,
        analyzer: Analyzer,
        rng: np.random.Generator,
        media: Media,
        send: Callable[[bytes], None],
    ) -> None:
        self.analyzer = analyzer
        self.rng = rng
        self.media = media
        self.send = send
        self.publisher = Publisher()
        self.beat = BeatSignal(analyzer.freqs, SAMPLERATE)
        self.activity = Activity()
        self.revision = -1  # of the configuration applied; -1 before the first
        self._surfaces: list[_Assigned] = []
        self._beat_layers = False
        self._track = 0
        # The front frames: what the last step rendered, swapped as a whole
        self.frames: dict[int, Frame] = {}
        self.timings: dict[int, dict[str, float]] = {}  # per surface: seconds per effect
        self.sequence = 0

    @property
    def configured(self) -> bool:
        return self.revision >= 0

    def apply(self, params: Mapping[str, Any]) -> None:
        """
        Takes a clamped `config.apply` (source-link.json's ConfigApplyParams).
        A surface index already rendered keeps its renderer, so its layers
        keep their state.
        """
        scenes = {
            str(scene['id']): scene
            for scene in cast(Sequence[Mapping[str, Any]], params.get('scenes', []))
        }
        old = {assigned.index: assigned.renderer for assigned in self._surfaces}
        surfaces: list[_Assigned] = []
        for entry in cast(Sequence[Mapping[str, Any]], params.get('surfaces', [])):
            index = int(entry['index'])
            scene = scenes.get(str(entry['scene']), {})
            by_role = cast(
                Mapping[str, Sequence[Mapping[str, Any]]], scene.get('layers_by_role', {})
            )
            renderer = old.get(index) or SurfaceRenderer(self.analyzer, self.rng)
            surfaces.append(
                _Assigned(
                    index,
                    cast(Mapping[str, Any], entry['surface']),
                    list(by_role.get(str(entry['role']), [])),
                    int(entry.get('beat_offset_ms', 0)),
                    bool(entry.get('album_colors', False)),
                    renderer,
                )
            )
        self._surfaces = surfaces
        self._beat_layers = any(_uses_beat(a.layers) for a in surfaces)
        self.beat.keep({a.beat_offset_ms: float(a.beat_offset_ms) for a in surfaces})
        self.publisher.configure(cast(Mapping[str, Any], params.get('publish', {})))
        self.publisher.unknown_signals(SIGNALS)
        self.revision = int(params.get('revision', 0))

    @property
    def beat_wanted(self) -> bool:
        """The beat tracker runs while a layer uses the beat or its signal is published."""
        return self._beat_layers or self.publisher.wants('beat')

    def step(self, steps: Sequence[Step], now_us: int) -> None:
        """
        Renders one frame of every surface from the audio of `steps` (more
        than one when steps were skipped to keep up), then publishes them.
        """
        if not self.configured or not steps:
            return
        self._follow_track()
        tracking = self.beat_wanted
        for audio in steps:
            if tracking:
                self.beat.push(audio.spectrum, audio.frames)
            else:
                self.beat.skip()
            self.activity.update(audio.spectrum, audio.dt)
        spectrum = steps[-1].spectrum
        dt = min(MAX_DT, sum(audio.dt for audio in steps))
        frames: dict[int, Frame] = {}
        timings: dict[int, dict[str, float]] = {}
        colors = self.media.colors
        for assigned in self._surfaces:
            tracker = self.beat.trackers.get(assigned.beat_offset_ms)
            frame, _ = assigned.renderer.render(
                assigned.surface,
                assigned.layers,
                spectrum,
                dt,
                now_us,
                self.sequence,
                beat=tracker,
                colors=colors if assigned.album_colors else (),
            )
            frames[assigned.index] = frame
            timings[assigned.index] = dict(assigned.renderer.timings)
        self.frames, self.timings = frames, timings
        self.sequence += 1
        self._publish(frames, now_us)

    def _follow_track(self) -> None:
        track = self.media.track
        if track != self._track:
            # A new song: every tracker finds its tempo afresh
            self._track = track
            self.beat.reset()

    def _publish(self, frames: Mapping[int, Frame], now_us: int) -> None:
        for index, frame in frames.items():
            data = self.publisher.surface(index, frame)
            if data is not None:
                self.send(data)
        signals = (
            Signal('beat', self.beat.values(), now_us, self.sequence),
            Signal('activity', self.activity.values(), now_us, self.sequence),
        )
        for signal in signals:
            if signal.name == 'beat' and not self.beat_wanted:
                continue
            data = self.publisher.signal(signal)
            if data is not None:
                self.send(data)


def _uses_beat(layers: Sequence[Mapping[str, Any]]) -> bool:
    """Whether an enabled audio-visualizer layer flashes on the beat."""
    for layer in layers:
        effect = cast(Mapping[str, Any], layer.get('effect', {}))
        params = cast(Mapping[str, Any], layer.get('params', {}))
        if (
            effect.get('id') == EFFECT_ID
            and layer.get('enabled', True)
            and params.get('trigger') == 'beat'
        ):
            return True
    return False
