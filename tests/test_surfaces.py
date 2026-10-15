"""
Surfaces and the effect interface (RB-04 steps 4 and 5): regions, axis
modes, blend modes and the stack, as docs/architecture/pipeline.md defines
them, and the audio visualizer in the effect registry.
"""

from __future__ import annotations

import math
from typing import Any, cast

import numpy as np
import pytest

from folo_source.contracts import clamp, models
from folo_source.effects import base
from folo_source.effects.visualizers import EFFECT_ID, AudioVisualizer
from folo_source.inputs.audio.analyzer import Analyzer, Spectrum, channel_peaks
from folo_source.legacy.renderer import PARAMS
from folo_source.surfaces.renderer import (
    SurfaceRenderer,
    axis_line,
    blend,
    canvas_cells,
    strip_cells,
)
from tests import golden_cases, synth


def test_strip_regions() -> None:
    assert strip_cells({'kind': 'cells', 'start': 10, 'end': 19}, 150) == (10, 20)
    assert strip_cells({'kind': 'cells', 'start': 140, 'end': 999}, 150) == (140, 150)
    first, stop = strip_cells({'kind': 'cells', 'start': 160, 'end': 170}, 150)
    assert stop <= first  # outside the strip: empty
    assert strip_cells({'kind': 'relative', 'start': 0.0, 'end': 1.0}, 150) == (0, 150)
    assert strip_cells({'kind': 'relative', 'start': 0.25, 'end': 0.5}, 10) == (2, 5)
    assert strip_cells({'kind': 'relative', 'start': 0.5, 'end': 0.5}, 10) == (5, 6)  # at least one
    assert strip_cells({'kind': 'rect', 'x': 0.5, 'width': 0.5, 'y': 0, 'height': 1}, 10) == (5, 10)


def test_canvas_regions() -> None:
    rect = {'kind': 'rect', 'x': 0.25, 'y': 0.5, 'width': 0.5, 'height': 0.5}
    assert canvas_cells(rect, 64, 36) == (18, 36, 16, 48)
    assert canvas_cells({'kind': 'relative', 'start': 0.0, 'end': 0.5}, 64, 36) == (0, 36, 0, 32)


def test_linear_axes() -> None:
    length, index = axis_line('linear_x', 3, 5)
    assert length == 5
    assert index.tolist() == [[0, 1, 2, 3, 4]] * 3
    length, index = axis_line('linear_y', 3, 5)
    assert length == 3
    assert index.tolist() == [[0] * 5, [1] * 5, [2] * 5]


def test_radial_axis() -> None:
    length, index = axis_line('radial', 9, 9)
    assert length == round(math.hypot(9, 9) / 2)
    assert index[4, 4] == 0  # the centre
    assert index[0, 0] == index[8, 8] == index[0, 8] == length - 1  # the corners
    assert index[4, 0] == index[0, 4]  # as far from the centre: the same cell


def test_angular_axis() -> None:
    length, index = axis_line('angular', 9, 9)
    assert length == 36
    assert index[0, 4] == 0  # straight up from the centre: where it starts
    assert index[4, 8] == 9  # a quarter turn clockwise, on the right
    assert index[8, 4] == 18  # half a turn, below
    assert index[4, 0] == 27  # three quarters, on the left


def test_blend_modes() -> None:
    def blended(mode: str, below: list[float], layer: list[float]) -> list[float]:
        out = np.array([below], dtype=np.float64)
        blend(mode, out, np.array([layer], dtype=np.float64))
        return out[0].tolist()

    assert blended('max', [10, 200, 50], [100, 100, 100]) == [100, 200, 100]
    assert blended('add', [10, 200, 50], [100, 100, 100]) == [110, 255, 150]
    assert blended('multiply', [255, 128, 0], [255, 255, 255]) == [255, 128, 0]
    assert blended('multiply', [200, 200, 200], [0, 127.5, 255]) == [0, 100, 200]
    assert blended('alpha', [10, 20, 30], [0, 0, 0]) == [10, 20, 30]  # black: transparent
    assert blended('alpha', [10, 20, 30], [255, 0, 0]) == [255, 0, 0]  # full colour: opaque
    assert blended('alpha', [100, 100, 100], [0, 51, 0]) == pytest.approx([80, 131, 80])  # 20%


WHOLE = {'kind': 'relative', 'start': 0.0, 'end': 1.0}


def surface_of(raw: dict[str, Any]) -> dict[str, Any]:
    return cast(dict[str, Any], clamp.clamp('Surface', raw))


def layer(layer_id: str, region: dict[str, Any], **fields: Any) -> dict[str, Any]:
    """A v2 audio-visualizer layer at full brightness (idle glow 1)."""
    params = {'floor': 1.0, 'mode': 'pulse', 'palette': ['#FF0000']}
    params.update(fields.pop('params', {}))
    raw = {'id': layer_id, 'effect': {'id': EFFECT_ID}, 'region': region, 'params': params}
    raw.update(fields)
    return cast(dict[str, Any], clamp.clamp('Layer', raw))


def strip(width: int, **fields: Any) -> dict[str, Any]:
    surface: dict[str, Any] = {'kind': 'strip', 'resolution': {'width': width, 'height': 1}}
    surface.update(fields)
    return surface_of(surface)


def compose(surface: dict[str, Any], layers: list[dict[str, Any]]) -> np.ndarray[Any, Any]:
    renderer = SurfaceRenderer(Analyzer(), rng=np.random.default_rng(1))
    out, _ = renderer.compose(surface, layers, Spectrum.silent(), 0.01)
    return renderer.finish(out, float(surface['master']))


def test_the_stack() -> None:
    cells = {'kind': 'cells', 'start': 2, 'end': 5}
    red = layer('red', cells)
    out = compose(strip(8, background='#000010'), [red])
    assert out[0].tolist() == [0, 0, 16]  # the background
    assert out[2].tolist() == out[5].tolist() == [255, 0, 16]  # max over the background
    assert out[6].tolist() == [0, 0, 16]
    # Solo: only solo layers draw; disabled ones never
    blue = layer('blue', {'kind': 'cells', 'start': 0, 'end': 7}, params={'palette': ['#0000FF']})
    assert compose(strip(8), [red, dict(blue, solo=True)])[3].tolist() == [0, 0, 255]
    assert compose(strip(8), [dict(red, enabled=False)])[3].tolist() == [0, 0, 0]
    # Master scales, then values are clipped
    assert compose(strip(8, master=0.5), [red])[3].tolist() == [127.5, 0, 0]
    # Order matters for alpha: the top layer covers what is below
    over = dict(blue, blend='alpha')
    assert compose(strip(8), [red, over])[3].tolist() == [0, 0, 255]
    # An effect the engine doesn't have draws nothing
    unknown = dict(red, effect={'id': 'no-such-effect', 'version': '1.0.0'})
    assert compose(strip(8), [unknown])[3].tolist() == [0, 0, 0]


def test_a_1d_effect_on_a_canvas_follows_its_axis() -> None:
    """A layer down a canvas's rows draws, in every column, what it draws on a strip."""
    params: dict[str, Any] = {'mode': 'fill', 'floor': 0.0, 'freq_lo': 500, 'freq_hi': 2000}
    params['threshold'] = -90
    params['palette'] = ['#0033CC', '#FF3399']
    on_strip = layer('fill', WHOLE, params=params)
    rect = {'kind': 'rect', 'x': 0, 'y': 0, 'width': 1, 'height': 1}
    on_canvas = layer('fill', rect, params=params, axis='linear_y')
    canvas = surface_of({'kind': 'canvas', 'resolution': {'width': 4, 'height': 12}})
    analyzer = Analyzer()
    mono = synth.tone(1000.0, 0.3, 0.3, 10.0)
    spectrum = Spectrum.silent()
    for frames in synth.packets(synth.capture(np.column_stack((mono, mono)))):
        spectrum = analyzer.push(frames, channel_peaks(frames))
    line, _ = SurfaceRenderer(Analyzer()).compose(strip(12), [on_strip], spectrum, 0.3)
    block, _ = SurfaceRenderer(Analyzer()).compose(canvas, [on_canvas], spectrum, 0.3)
    assert line.max() > 0, 'the tone lights the layer'
    for column in range(4):
        assert np.array_equal(block[:, column], line)


def test_frames_truncate_to_the_stacks_bytes() -> None:
    surface = strip(30)
    layers = [layer('fill', WHOLE, params={'mode': 'fill', 'floor': 0.1})]
    framed, plain = SurfaceRenderer(Analyzer()), SurfaceRenderer(Analyzer())
    analyzer = Analyzer()
    for sequence, frames in enumerate(list(synth.packets(golden_cases.signal('medley')))[:300]):
        spectrum = analyzer.push(frames, channel_peaks(frames))
        frame, levels = framed.render(surface, layers, spectrum, 0.01, 0, sequence)
        out, _ = plain.compose(surface, layers, spectrum, 0.01)
        plain.finish(out, 1.0)
        assert np.array_equal(frame.pixels.astype(np.uint8), out.astype(np.uint8))
        assert frame.width == 30
        assert set(levels) == {'fill'}


def test_the_effect_registry() -> None:
    assert EFFECT_ID in base.effect_ids()
    effect = base.create(EFFECT_ID, Analyzer(), np.random.default_rng(1))
    assert isinstance(effect, AudioVisualizer)
    assert base.create('no-such-effect', Analyzer(), np.random.default_rng(1)) is None
    info = models.EffectInfo.model_validate(effect.info.as_dict())
    assert info.id == 'audio-visualizer'
    assert info.params_schema == clamp.definition('AudioVisualizerParams')


def test_segment_fields_are_the_effects_parameters() -> None:
    assert tuple(models.AudioVisualizerParams.model_fields) == PARAMS


@pytest.mark.parametrize('axis', ['linear_x', 'linear_y', 'radial', 'angular'])
def test_every_canvas_cell_takes_a_line_cell(axis: str) -> None:
    length, index = axis_line(axis, 7, 11)
    assert index.shape == (7, 11)
    assert index.min() >= 0 and index.max() < length
