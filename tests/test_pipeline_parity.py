"""
Parity mode (RB-04 step 9, ADR-0005 D8): today's configs, as v2 layers on a
strip surface with 1:1 cells, clamped like any v2 document, render through
the new pipeline to the RB-01 golden frames byte for byte, beat-triggered
layers included. The ambient light moves to the controller (RB-08), so the
cases that show it are checked there.
"""

from __future__ import annotations

from typing import Any, cast

import numpy as np
import pytest

from folo_source.contracts import clamp
from folo_source.core import now_us
from folo_source.inputs.audio.analyzer import SAMPLERATE, Analyzer, Spectrum
from folo_source.legacy.renderer import layers_from_v1, surface_from_v1
from folo_source.signals.beat import BeatSignal
from folo_source.surfaces.renderer import SurfaceRenderer
from tests import golden_cases as g

MANIFEST = g.load_manifest()
CASES = {
    name: case for name, case in g.by_name(g.all_cases()).items() if case.config['reactive'] == 'on'
}
ORDER = [c.name for c in sorted(CASES.values(), key=lambda c: (c.analysis, c.name))]


def test_most_cases_are_covered() -> None:
    assert len(CASES) > 0.9 * len(MANIFEST['cases'])


@pytest.mark.parametrize('name', ORDER)
def test_strip_parity(name: str) -> None:
    case = CASES[name]
    config = case.config
    surface = cast(dict[str, Any], clamp.clamp('Surface', surface_from_v1(config)))
    layers = [cast(dict[str, Any], clamp.clamp('Layer', layer)) for layer in layers_from_v1(config)]
    analyzer = Analyzer()
    renderer = SurfaceRenderer(analyzer, rng=np.random.default_rng(g.SEED))
    beat = BeatSignal(analyzer.freqs, SAMPLERATE)
    beat.keep({'zone': config['beat']['offset']})
    tracker = beat.trackers['zone']
    tracking = g.uses_beat(config)
    colors = tuple(case.colors)
    frames: list[g.Frames] = []

    def step(spectrum: Spectrum, count: int, dt: float) -> None:
        if tracking:
            beat.push(spectrum, count)
        else:
            beat.skip()
        frame, _ = renderer.render(
            surface, layers, spectrum, dt, now_us(), len(frames), beat=tracker, colors=colors
        )
        frames.append(frame.pixels.astype(np.uint8))

    for packet in g.analyse(case.analysis):
        step(packet.spectrum, packet.frames, packet.dt)
    for dt in g.idle_steps(case.idle):
        step(Spectrum.silent(), int(dt * SAMPLERATE), dt)
    actual = np.stack(frames)
    expected = g.read_png(g.RENDERER_DIR / MANIFEST['cases'][name]['png'])
    assert actual.shape == expected.shape
    differ = np.flatnonzero((actual != expected).any(axis=(1, 2)))
    assert not differ.size, f'{name}: frames differ from frame {int(differ[0])}'
