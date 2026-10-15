"""
The post-processing layers (RB-04 step 6): brightness and saturation,
mirror, blur and trail, as docs/architecture/pipeline.md#effects defines
them, alone and in a stack.
"""

from __future__ import annotations

import math
from typing import Any, cast

import numpy as np
import pytest

from folo_source.contracts import clamp, models
from folo_source.effects import base
from folo_source.effects.base import LayerSpec, PostEffect
from folo_source.effects.post import Blur, Brightness, Mirror, Trail
from folo_source.inputs.audio.analyzer import Analyzer, Spectrum
from folo_source.surfaces.renderer import SurfaceRenderer


def apply(
    effect: PostEffect, cells: list[list[float]], dt: float = 0.01, **params: Any
) -> np.ndarray[Any, Any]:
    block = np.array(cells, dtype=np.float64)
    effect.prepare([LayerSpec('post', params, len(block), True, params.pop('axis', 'linear_x'))])
    effect.apply(0, block, dt)
    return block


def test_brightness_and_saturation() -> None:
    cells = [[200.0, 100.0, 50.0], [10.0, 20.0, 30.0]]
    assert apply(Brightness(), cells).tolist() == cells  # the defaults change nothing
    grey = apply(Brightness(), cells, saturation=0.0)
    luma = np.array(cells) @ np.array([0.2126, 0.7152, 0.0722])
    assert np.allclose(grey, np.repeat(luma[:, None], 3, axis=1))
    assert apply(Brightness(), cells, brightness=2.0).tolist() == [[255, 200, 100], [20, 40, 60]]
    vivid = apply(Brightness(), [[100.0, 120.0, 140.0]], saturation=3.0)
    assert vivid[0, 2] - vivid[0, 0] > 100  # further apart than before (40)
    assert (apply(Brightness(), [[250.0, 0.0, 0.0]], saturation=4.0) >= 0).all()  # clipped


def column(values: list[float]) -> list[list[float]]:
    return [[v, v, v] for v in values]


def test_mirror_on_a_strip() -> None:
    mirrored = apply(Mirror(), column([1, 2, 3, 4, 5]))[:, 0].tolist()
    assert mirrored == [1, 2, 3, 2, 1]  # the middle cell stays
    assert apply(Mirror(), column([1, 2, 3, 4]), keep='second')[:, 0].tolist() == [4, 3, 3, 4]
    assert apply(Mirror(), column([7]))[:, 0].tolist() == [7]


def test_mirror_on_a_canvas_follows_the_axis() -> None:
    canvas = np.arange(2 * 4, dtype=np.float64).reshape(2, 4)[..., None].repeat(3, axis=2)
    across = Mirror()
    across.prepare([LayerSpec('m', {}, 4, True, 'linear_x')])
    block = canvas.copy()
    across.apply(0, block, 0.01)
    assert block[..., 0].tolist() == [[0, 1, 1, 0], [4, 5, 5, 4]]
    down = Mirror()
    down.prepare([LayerSpec('m', {'keep': 'second'}, 2, True, 'linear_y')])
    block = canvas.copy()
    down.apply(0, block, 0.01)
    assert block[..., 0].tolist() == [[4, 5, 6, 7], [4, 5, 6, 7]]


def test_blur() -> None:
    assert apply(Blur(), column([0, 0, 90, 0, 0]))[:, 0].tolist() == [0, 30, 30, 30, 0]
    # Past the edges, the edge cells repeat
    assert apply(Blur(), column([90, 0, 0]))[:, 0].tolist() == [60, 30, 0]
    assert apply(Blur(), column([0, 0, 0, 90, 0, 0, 0]), radius=2)[:, 0].tolist() == pytest.approx(
        [0, 18, 18, 18, 18, 18, 0]
    )
    # On a canvas: the square around each cell
    canvas = np.zeros((5, 5, 3))
    canvas[2, 2] = 90.0
    blur = Blur()
    blur.prepare([LayerSpec('b', {'radius': 1}, 5, True)])
    blur.apply(0, canvas, 0.01)
    assert canvas[1:4, 1:4, 0] == pytest.approx(np.full((3, 3), 10.0))
    assert canvas[0, :, 0].tolist() == [0, 0, 0, 0, 0]


def test_trail() -> None:
    trail = Trail()
    trail.prepare([LayerSpec('t', {'fade': 100}, 3, True)])
    bright = np.full((3, 3), 200.0)
    trail.apply(0, bright, 0.01)
    assert bright.tolist() == [[200.0] * 3] * 3  # the first frame passes through
    dark = np.zeros((3, 3))
    trail.apply(0, dark, 0.05)
    assert dark[0, 0] == pytest.approx(200.0 * math.exp(-0.05 / 0.1))
    louder = np.full((3, 3), 250.0)
    trail.apply(0, louder, 0.05)
    assert louder[0, 0] == 250.0  # never darker than what is below
    resized = np.zeros((4, 3))
    trail.apply(0, resized, 0.05)
    assert resized.max() == 0.0  # a new size starts over


def layer(effect: str, region: dict[str, Any], **fields: Any) -> dict[str, Any]:
    raw = {'id': fields.pop('id', effect), 'effect': {'id': effect}, 'region': region}
    raw.update(fields)
    return cast(dict[str, Any], clamp.clamp('Layer', raw))


def test_post_layers_in_a_stack() -> None:
    whole = {'kind': 'relative', 'start': 0.0, 'end': 1.0}
    pulse = {'mode': 'pulse', 'floor': 1.0, 'palette': ['#C80000']}
    red = layer('audio-visualizer', {'kind': 'cells', 'start': 2, 'end': 2}, id='red', params=pulse)
    blur = layer('blur', {'kind': 'cells', 'start': 0, 'end': 3}, blend='add')
    surface = cast(
        dict[str, Any],
        clamp.clamp('Surface', {'kind': 'strip', 'resolution': {'width': 6, 'height': 1}}),
    )
    renderer = SurfaceRenderer(Analyzer())
    out, _ = renderer.compose(surface, [red, blur], Spectrum.silent(), 0.01)
    # Blurred inside its region only, and its blend mode doesn't apply
    assert out[:, 0].tolist() == pytest.approx([0, 200 / 3, 200 / 3, 200 / 3, 0, 0])
    # A post layer only changes what is below it
    out, _ = SurfaceRenderer(Analyzer()).compose(surface, [blur, red], Spectrum.silent(), 0.01)
    assert out[:, 0].tolist() == [0, 0, 200, 0, 0, 0]
    dimmed = layer('brightness', whole, params={'brightness': 0.5})
    out, _ = SurfaceRenderer(Analyzer()).compose(surface, [red, dimmed], Spectrum.silent(), 0.01)
    assert out[2].tolist() == [100, 0, 0]


def test_post_layer_parameters_are_clamped() -> None:
    blur = layer('blur', {'kind': 'relative'}, params={'radius': 99, 'colour': 'red'})
    assert blur['params'] == {'radius': 16}
    trail = layer('trail', {'kind': 'relative'}, params={'fade': 'slow'})
    assert trail['params'] == {'fade': 300}
    mirror = layer('mirror', {'kind': 'relative'}, params={'keep': 'middle'})
    assert mirror['params'] == {'keep': 'first'}


@pytest.mark.parametrize(
    ('effect_id', 'definition'),
    [
        ('brightness', 'BrightnessParams'),
        ('mirror', 'MirrorParams'),
        ('blur', 'BlurParams'),
        ('trail', 'TrailParams'),
    ],
)
def test_post_effects_are_registered(effect_id: str, definition: str) -> None:
    effect = base.create(effect_id, Analyzer(), np.random.default_rng(1))
    assert isinstance(effect, PostEffect)
    info = models.EffectInfo.model_validate(effect.info.as_dict())
    assert info.id == effect_id
    assert info.inputs == []
    assert info.params_schema == clamp.definition(definition)
