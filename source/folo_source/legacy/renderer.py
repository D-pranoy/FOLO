"""
The legacy studio's renderer: today's v1 config drawn through the new
pipeline, its segments as audio-visualizer layers on one strip surface (the
mapping in docs/specs/config-model.md#migration-from-the-current-studiojson-version-1),
with the ambient light, which stays here until the controller takes it over
(RB-08).
"""

from collections.abc import Sequence
from typing import Any

import numpy as np

from folo_source.effects.base import Levels
from folo_source.effects.visualizers import EFFECT_ID, VERSION
from folo_source.inputs.audio.analyzer import Analyzer, Spectrum
from folo_source.legacy.ambient import Ambient
from folo_source.surfaces.renderer import SurfaceRenderer

# A segment's fields that are the audio visualizer's parameters, unchanged
# (show.json's AudioVisualizerParams)
PARAMS = (
    'freq_lo',
    'freq_hi',
    'trigger',
    'beats',
    'threshold',
    'ceiling',
    'gain',
    'curve',
    'attack',
    'release',
    'floor',
    'mode',
    'direction',
    'speed',
    'width',
    'peak',
    'palette',
)


def surface_from_v1(config: dict[str, Any]) -> dict[str, Any]:
    """The strip surface of a v1 config: its LEDs, background and master."""
    return {
        'id': 'main',
        'kind': 'strip',
        'resolution': {'width': config['led_count'], 'height': 1},
        'role': 'strip.main',
        'background': config['background'],
        'master': config['master'],
    }


def layers_from_v1(config: dict[str, Any]) -> list[dict[str, Any]]:
    """A v1 config's segments as audio-visualizer layers, in the same order."""
    return [
        {
            'id': seg['id'],
            'name': seg['name'],
            'effect': {'id': EFFECT_ID, 'version': VERSION},
            'params': {key: seg[key] for key in PARAMS},
            'region': {'kind': 'cells', 'start': seg['start'], 'end': seg['end']},
            'blend': 'max',
            'axis': 'linear_x',
            'enabled': seg['enabled'],
            'solo': seg['solo'],
            'lane': seg['lane'],
        }
        for seg in config['segments']
    ]


class Renderer:
    """
    Turns the spectrum into LED colours according to a v1 config's segments,
    and blends in the ambient light.
    """

    def __init__(self, analyzer: Analyzer, rng: np.random.Generator | None = None) -> None:
        self.analyzer = analyzer
        self.ambient = Ambient()
        self.surface = SurfaceRenderer(analyzer, rng)
        self._config: dict[str, Any] | None = None
        self._strip: dict[str, Any] = {}
        self._layers: list[dict[str, Any]] = []

    @property
    def rng(self) -> np.random.Generator:
        return self.surface.rng

    @rng.setter
    def rng(self, rng: np.random.Generator) -> None:
        self.surface.rng = rng

    def render(
        self,
        config: dict[str, Any],
        spectrum: Spectrum,
        dt: float,
        beat: Any = None,
        colors: Sequence[str] = (),
        paused: bool = False,
    ) -> tuple[bytes, Levels]:
        """
        Returns the LED colours as bytes and
        {segment id: (level dB, value, recent average dB)}, unrounded (see
        shown_levels). `beat`: a BeatTracker pushed with the same audio (beat
        triggers stay dark without one). `colors`: the album art's colours,
        used instead of the palettes. `paused`: the player is paused.
        """
        if config is not self._config:
            # The store replaces the config on every change and never edits it
            self._config = config
            self._strip = surface_from_v1(config)
            self._layers = layers_from_v1(config)
        out, levels = self.surface.compose(
            self._strip, self._layers, spectrum, dt, beat, tuple(colors)
        )
        out = self.ambient.blend(out, config, spectrum, dt, paused)
        SurfaceRenderer.finish(out, config['master'])
        return out.astype(np.uint8).tobytes(), levels
