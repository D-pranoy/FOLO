"""
The executor (RB-04 step 8, docs/architecture/pipeline.md#the-executor):
renders the surfaces `config.apply` assigned from the analysed audio and
publishes them. Each step works in dependency order: every analysed packet
goes to the beat signal and the activity, then each surface's stack is
rendered bottom first, then the frames and signals are published. A
surface with Screen layers also has an activity of its own, from the
screen input (ADR-0037 D12).
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Protocol, cast

import numpy as np

from folo_source.core import Frame, Signal
from folo_source.effects.base import EffectInputs, Levels, shown_levels
from folo_source.effects.screen import EFFECT_ID as SCREEN_EFFECT_ID
from folo_source.effects.visualizers import EFFECT_ID
from folo_source.inputs.audio.analyzer import QUIET_DB, SAMPLERATE, Analyzer, Spectrum
from folo_source.inputs.audio.input import MAX_DT
from folo_source.inputs.screen import ScreenInput
from folo_source.publish.publisher import Publisher
from folo_source.signals.beat import BeatSignal
from folo_source.surfaces.renderer import SurfaceRenderer

# The signals this engine has
SIGNALS = ('beat', 'activity', 'spectrum')
# The spectrum and the layers' levels go out at most 60 times a second (ADR-0035 D6)
DISPLAY_EVERY_US = 1_000_000 // 60

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
    screen: bool = False  # it has an enabled Screen layer
    audio: bool = False  # it has an enabled audio-visualizer layer
    counted_from_us: int | None = None  # where its Screen activity's silence counts from


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
        screen: ScreenInput | None = None,
    ) -> None:
        self.analyzer = analyzer
        self.rng = rng
        self.media = media
        self.send = send
        self.screen = ScreenInput('') if screen is None else screen
        self.inputs = EffectInputs(screen=self.screen)
        self.publisher = Publisher()
        self.beat = BeatSignal(analyzer.freqs, SAMPLERATE)
        self.activity = Activity()
        self.revision = -1  # of the configuration applied; -1 before the first
        self._surfaces: list[_Assigned] = []
        self._beat_layers = False
        self._track = 0
        # The front frames: what the last step rendered, swapped as a whole
        self.frames: dict[int, Frame] = {}
        self.levels: dict[int, Levels] = {}  # per surface: its layers' levels
        # The spectrum signal's bands (docs/specs/source-link.md#signal-frames)
        self._display_plan = analyzer.band_plan(analyzer.display_edges)
        self._display_due: dict[str, int] = {}  # when the spectrum and levels may go next
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
        old = {assigned.index: assigned for assigned in self._surfaces}
        surfaces: list[_Assigned] = []
        for entry in cast(Sequence[Mapping[str, Any]], params.get('surfaces', [])):
            index = int(entry['index'])
            scene = scenes.get(str(entry['scene']), {})
            by_role = cast(
                Mapping[str, Sequence[Mapping[str, Any]]], scene.get('layers_by_role', {})
            )
            before = old.get(index)
            renderer = (
                before.renderer
                if before is not None
                else SurfaceRenderer(self.analyzer, self.rng, self.inputs)
            )
            layers = list(by_role.get(str(entry['role']), []))
            surfaces.append(
                _Assigned(
                    index,
                    cast(Mapping[str, Any], entry['surface']),
                    layers,
                    int(entry.get('beat_offset_ms', 0)),
                    bool(entry.get('album_colors', False)),
                    renderer,
                    _uses(layers, SCREEN_EFFECT_ID),
                    _uses(layers, EFFECT_ID),
                    before.counted_from_us if before is not None else None,
                )
            )
        self._surfaces = surfaces
        self._beat_layers = any(_uses_beat(a.layers) for a in surfaces)
        self.beat.keep({a.beat_offset_ms: float(a.beat_offset_ms) for a in surfaces})
        self.publisher.configure(cast(Mapping[str, Any], params.get('publish', {})))
        self.publisher.unknown_signals(SIGNALS)
        self.revision = int(params.get('revision', 0))

    @property
    def screen_wanted(self) -> bool:
        """Whether a surface has an enabled Screen layer, so a new frame is worth a step."""
        return any(assigned.screen for assigned in self._surfaces)

    def screen_activity(self, now_us: int) -> list[dict[str, Any]]:
        """
        The activity of each surface with a Screen layer (ADR-0037 D12), as
        notification params: active while the screen input is, or the audio
        is when the surface has audio layers too. No `level_db`.
        """
        found: list[dict[str, Any]] = []
        for assigned in self._surfaces:
            if not assigned.screen:
                continue
            if assigned.counted_from_us is None:
                assigned.counted_from_us = now_us
            active = self.screen.active(now_us)
            quiet = self.screen.inactive_since(now_us, assigned.counted_from_us)
            if assigned.audio:
                active = active or self.activity.active
                quiet = min(quiet, self.activity.quiet_s)
            found.append(
                {
                    'surface': assigned.index,
                    'active': active,
                    'quiet_s': 0.0 if active else round(quiet, 2),
                }
            )
        return found

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
        levels: dict[int, Levels] = {}
        timings: dict[int, dict[str, float]] = {}
        colors = self.media.colors
        for assigned in self._surfaces:
            tracker = self.beat.trackers.get(assigned.beat_offset_ms)
            frame, levels[assigned.index] = assigned.renderer.render(
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
        self.frames, self.levels, self.timings = frames, levels, timings
        self.sequence += 1
        self._publish(frames, spectrum, now_us)

    def _follow_track(self) -> None:
        track = self.media.track
        if track != self._track:
            # A new song: every tracker finds its tempo afresh
            self._track = track
            self.beat.reset()

    def _display_due_now(self, what: str, now_us: int) -> bool:
        """Whether the spectrum or the levels may go out again."""
        if now_us < self._display_due.get(what, 0):
            return False
        self._display_due[what] = now_us + DISPLAY_EVERY_US
        return True

    def levels_due(self, now_us: int) -> list[dict[str, Any]]:
        """
        The levels notifications due after a step: one per surface in
        `publish.levels`, at most 60 times a second.
        """
        wanted = [i for i in self.publisher.levels if i in self.levels]
        if not wanted or not self._display_due_now('levels', now_us):
            return []
        return [{'surface': i, 'layers': shown_levels(self.levels[i])} for i in wanted]

    def _publish(self, frames: Mapping[int, Frame], spectrum: Spectrum, now_us: int) -> None:
        for index, frame in frames.items():
            data = self.publisher.surface(index, frame)
            if data is not None:
                self.send(data)
        signals = [
            Signal('beat', self.beat.values(), now_us, self.sequence),
            Signal('activity', self.activity.values(), now_us, self.sequence),
        ]
        if self.publisher.wants('spectrum') and self._display_due_now('spectrum', now_us):
            bands = self.analyzer.planned_levels(spectrum, self._display_plan)
            values = tuple(float(band) for band in bands)
            signals.append(Signal('spectrum', values, now_us, self.sequence))
        for signal in signals:
            if signal.name == 'beat' and not self.beat_wanted:
                continue
            data = self.publisher.signal(signal)
            if data is not None:
                self.send(data)


def _uses(layers: Sequence[Mapping[str, Any]], effect_id: str) -> bool:
    """Whether an enabled layer runs the effect `effect_id`."""
    return any(
        layer.get('enabled', True)
        and cast(Mapping[str, Any], layer.get('effect', {})).get('id') == effect_id
        for layer in layers
    )


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
