"""
The screen input and the Screen effect (RB-14 step 7, ADR-0037 D8, D12 and
D13), step by step with small synthetic frames and exact expected values:
the held and protected frames, the crop, area and edge sampling, colour,
smoothing, and the canvas on a surface.
"""

from __future__ import annotations

import math
import time
from typing import Any

import numpy as np
import pytest

from folo_source.effects.base import EffectInputs, LayerSpec
from folo_source.effects.screen import (
    Screen,
    Settings,
    band_weights,
    crop,
    margin_cells,
    sample_area,
    sample_edge,
)
from folo_source.inputs.audio.analyzer import Analyzer, Spectrum
from folo_source.inputs.screen import STALE_US, ScreenInput
from folo_source.publish.frames import area_weights, resample
from folo_source.surfaces.renderer import SurfaceRenderer
from tests import screen_frames as sf

RED, GREEN, BLUE, YELLOW = (255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)


class Clock:
    def __init__(self) -> None:
        self.now = 1_000_000

    def __call__(self) -> int:
        return self.now


def make(clock: Clock | None = None) -> tuple[ScreenInput, Screen]:
    screen = ScreenInput('display-1', enabled=True, clock=clock or Clock())
    return screen, Screen(inputs=EffectInputs(screen=screen))


def layer(
    rows: int, cols: int, visible: bool = True, layer_id: str = 's1', **params: Any
) -> LayerSpec:
    return LayerSpec(layer_id, params, cols, visible, 'linear_x', rows, cols)


def picture(*rows: list[tuple[int, int, int]]) -> np.ndarray:
    return np.array(rows, dtype=np.uint8)


def drawn(effect: Screen, dt: float = 0.01) -> np.ndarray:
    result = effect.draw(dt)[0]
    assert result is not None
    return result


# ---------------------------------------------------------------- the input


def test_the_input_keeps_the_last_usable_picture() -> None:
    clock = Clock()
    screen, _ = make(clock)
    assert screen.picture() is None
    first = sf.screen_frame(sf.solid(2, 2, (1, 2, 3)), sequence=1)
    screen.push(first)
    arrived = clock.now
    clock.now += 100_000
    screen.push(sf.screen_frame(sf.solid(2, 2, (9, 9, 9)), flags=sf.HELD, sequence=2))
    screen.push(sf.screen_frame(sf.solid(2, 2, (8, 8, 8)), flags=sf.PROTECTED, sequence=3))
    kept = screen.picture()
    assert kept is not None and kept.frame is first and kept.arrived_us == arrived
    newest = sf.screen_frame(sf.solid(2, 2, (4, 5, 6)), flags=sf.HDR, sequence=4)
    screen.push(newest)
    kept = screen.picture()
    assert kept is not None and kept.frame is newest and kept.arrived_us == clock.now


def test_it_is_active_for_half_a_second_after_a_usable_frame() -> None:
    clock = Clock()
    screen, _ = make(clock)
    assert not screen.active(clock.now)
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    assert screen.active(clock.now)
    assert screen.active(clock.now + STALE_US - 1)
    assert not screen.active(clock.now + STALE_US)


def test_held_and_protected_frames_do_not_keep_it_active() -> None:
    clock = Clock()
    screen, _ = make(clock)
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    for step in range(1, 10):
        clock.now += 100_000
        flag = sf.HELD if step % 2 else sf.PROTECTED
        screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3)), flags=flag))
    assert not screen.active(clock.now)
    assert screen.status()['held'] is True  # the newest was held
    assert screen.status()['protected'] is False
    assert screen.health() == 'waiting'


def test_seconds_inactive() -> None:
    clock = Clock()
    screen, _ = make(clock)
    start = clock.now
    clock.now += 2_000_000
    assert screen.inactive_since(clock.now, start) == 2.0  # never had a picture
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    clock.now += STALE_US + 1_500_000
    assert screen.inactive_since(clock.now, start) == pytest.approx(1.5)


def test_a_turned_off_input_keeps_nothing() -> None:
    screen, _ = make()
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    screen.configure('display-1', False)
    assert screen.picture() is None and screen.health() == 'idle'
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    assert screen.picture() is None
    screen.configure('display-1', True)
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    screen.configure('display-2', True)  # another screen: the picture is of the first
    assert screen.picture() is None


def test_the_status_holds_no_pixels() -> None:
    clock = Clock()
    screen, _ = make(clock)
    screen.push(sf.screen_frame(sf.solid(3, 5, (254, 1, 253)), bars=(1, 0, 2, 0), flags=sf.HDR))
    status = screen.status()
    assert status == {
        'id': 'display-1',
        'enabled': True,
        'state': 'ok',
        'rate': 1,
        'width': 5,
        'height': 3,
        'bars': [1, 0, 2, 0],
        'protected': False,
        'hdr': True,
        'held': False,
    }


def test_it_wakes_the_renderer() -> None:
    woken: list[int] = []
    screen = ScreenInput('display-1', enabled=True, on_frame=lambda: woken.append(1))
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    assert woken == [1]


# ---------------------------------------------------------------- the steps


def test_it_draws_nothing_without_a_picture() -> None:
    screen, effect = make()
    effect.prepare([layer(2, 2)])
    assert effect.draw(0.01) == [None]
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3)), flags=sf.PROTECTED))
    assert effect.draw(0.01) == [None]  # a layer that never had one
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    assert effect.draw(0.01)[0] is not None


def test_an_invisible_or_empty_layer_draws_nothing() -> None:
    screen, effect = make()
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 2, 3))))
    effect.prepare([layer(2, 2, visible=False, layer_id='a'), layer(0, 0, layer_id='b')])
    assert effect.draw(0.01) == [None, None]


def test_held_and_protected_pixels_are_ignored() -> None:
    screen, effect = make()
    effect.prepare([layer(1, 1, smoothing=0)])
    screen.push(sf.screen_frame(sf.solid(2, 2, (10, 20, 30))))
    assert drawn(effect).tolist() == [[[10, 20, 30]]]
    screen.push(sf.screen_frame(sf.solid(2, 2, (200, 1, 1)), flags=sf.HELD))
    screen.push(sf.screen_frame(sf.solid(2, 2, (1, 200, 1)), flags=sf.PROTECTED))
    assert drawn(effect).tolist() == [[[10, 20, 30]]]
    screen.push(sf.screen_frame(sf.solid(2, 2, (40, 50, 60))))
    assert drawn(effect).tolist() == [[[40, 50, 60]]]


def test_turning_the_screen_off_clears_the_layers() -> None:
    screen, effect = make()
    effect.prepare([layer(1, 1)])
    screen.push(sf.screen_frame(sf.solid(2, 2, (10, 20, 30))))
    drawn(effect)
    screen.configure('display-1', False)
    assert effect.draw(0.01) == [None]


def test_the_black_bars_are_cropped_with_follow() -> None:
    rows = [[(0, 0, 0)] * 4, [RED] * 4, [GREEN] * 4, [(0, 0, 0)] * 4]
    frame = sf.screen_frame(picture(*rows), bars=(1, 1, 0, 0))
    screen, effect = make()
    screen.push(frame)
    effect.prepare([layer(2, 4, smoothing=0)])
    assert drawn(effect).tolist() == [[list(RED)] * 4, [list(GREEN)] * 4]
    effect.prepare([layer(4, 4, smoothing=0, bars='ignore')])
    assert drawn(effect).tolist() == [
        [[0, 0, 0]] * 4,
        [list(RED)] * 4,
        [list(GREEN)] * 4,
        [[0, 0, 0]] * 4,
    ]


def test_the_columns_of_pillarbox_bars() -> None:
    row = [(0, 0, 0), RED, GREEN, BLUE, (0, 0, 0)]
    screen, effect = make()
    screen.push(sf.screen_frame(picture(row), bars=(0, 0, 1, 1)))
    effect.prepare([layer(1, 3, smoothing=0)])
    assert drawn(effect).tolist() == [[list(RED), list(GREEN), list(BLUE)]]


def test_margins_are_shares_of_what_remains() -> None:
    frame = sf.screen_frame(
        np.arange(10 * 10 * 3, dtype=np.uint8).reshape(10, 10, 3), bars=(2, 0, 0, 0)
    )
    wide = frame.pixels.astype(np.float64)
    shown = crop(frame, wide, Settings.of({'margins': {'top': 0.25, 'right': 0.2}}))
    # 8 rows remain after the bars; a quarter is 2 rows; a fifth of 10 columns is 2
    assert shown.shape == (6, 8, 3)
    assert np.array_equal(shown, wide[4:, :8])
    # with the bars ignored, a quarter of 10 rows is 2.5, which rounds up to 3
    ignored = crop(frame, wide, Settings.of({'bars': 'ignore', 'margins': {'top': 0.25}}))
    assert ignored.shape == (7, 10, 3)


def test_margins_leave_at_least_one_cell() -> None:
    assert margin_cells((0.45, 0.45), 2) == (1, 0)
    assert margin_cells((0.45, 0.45), 1) == (0, 0)
    assert margin_cells((0.25, 0.25), 8) == (2, 2)
    assert margin_cells((0.0, 0.0), 8) == (0, 0)
    assert margin_cells((0.1, 0.0), 5) == (1, 0)  # 0.5 cell rounds up


def test_area_sampling_averages_by_coverage() -> None:
    two = np.array([[[0, 0, 0], [200, 100, 50]]], dtype=np.float64)
    assert sample_area(two, 1, 1).tolist() == [[[100, 50, 25]]]
    three = sample_area(two, 1, 3)[0]
    assert three[0].tolist() == [0, 0, 0]
    assert three[1].tolist() == pytest.approx([100, 50, 25])
    assert three[2].tolist() == pytest.approx([200, 100, 50])
    quad = np.array([[[0, 0, 0], [4, 8, 12]], [[8, 16, 24], [12, 24, 36]]], dtype=np.float64)
    assert sample_area(quad, 1, 1)[0, 0].tolist() == pytest.approx([6, 12, 18])
    assert sample_area(quad, 2, 2).tolist() == quad.tolist()


def edge_picture() -> np.ndarray:
    """A 4 x 4 picture whose sides have colours of their own and whose middle is grey."""
    grid = np.full((4, 4, 3), 128, dtype=np.uint8)
    grid[0], grid[3] = RED, BLUE
    grid[1:3, 0], grid[1:3, 3] = GREEN, YELLOW
    return grid


def test_edge_sampling_takes_the_nearest_side() -> None:
    cells = sample_edge(edge_picture().astype(np.float64), 4, 4, 0.25)
    expected = {
        (0, 0): RED,
        (0, 3): RED,
        (1, 0): GREEN,
        (2, 3): YELLOW,
        (3, 1): BLUE,
        (3, 3): BLUE,
        (1, 1): RED,  # equally near the top and the left: the top wins
        (1, 2): RED,  # the top and the right
        (2, 1): BLUE,  # the bottom and the left: the bottom wins
        (2, 2): BLUE,
    }
    for (row, col), colour in expected.items():
        assert cells[row, col].tolist() == pytest.approx(list(colour)), (row, col)


def test_edge_sampling_spreads_the_band_over_the_cells_span() -> None:
    top = np.zeros((2, 4, 3))
    top[0, :, 0] = [0, 100, 200, 40]  # the red of the top row
    cells = sample_edge(top, 1, 2, 0.5)  # one row: every cell is nearest the top or bottom
    # The top band is row 0; two cells each take the average of two columns
    assert cells[0, 0, 0] == pytest.approx(50.0)
    assert cells[0, 1, 0] == pytest.approx(120.0)


def test_the_edge_band_depth() -> None:
    assert band_weights(4, 0.25).tolist() == [1, 0, 0, 0]
    assert band_weights(4, 0.5).tolist() == [0.5, 0.5, 0, 0]
    assert band_weights(4, 0.375).tolist() == pytest.approx([2 / 3, 1 / 3, 0, 0])
    assert band_weights(1, 0.02).tolist() == [1.0]
    grid = np.zeros((4, 1, 3))
    grid[:, 0, 0] = [0, 90, 255, 255]
    cells = sample_edge(grid, 1, 1, 0.5)  # the band is rows 0 and 1; top ties with the others
    assert cells[0, 0, 0] == pytest.approx(45.0)


def test_edge_mode_through_the_effect() -> None:
    screen, effect = make()
    screen.push(sf.screen_frame(edge_picture()))
    effect.prepare([layer(4, 4, mode='edge', edge_depth=0.25, smoothing=0)])
    result = drawn(effect)
    assert result[1, 0].tolist() == list(GREEN) and result[3, 2].tolist() == list(BLUE)


def test_saturation_and_brightness() -> None:
    screen, effect = make()
    screen.push(sf.screen_frame(sf.solid(2, 2, (200, 100, 0))))
    luma = 0.2126 * 200 + 0.7152 * 100
    effect.prepare([layer(1, 1, smoothing=0, saturation=0)])
    assert drawn(effect)[0, 0].tolist() == pytest.approx([luma] * 3)
    effect.prepare([layer(1, 1, smoothing=0, brightness=0.5)])
    assert drawn(effect)[0, 0].tolist() == pytest.approx([100, 50, 0])
    effect.prepare([layer(1, 1, smoothing=0, brightness=2)])
    assert drawn(effect)[0, 0].tolist() == pytest.approx([255, 200, 0])
    effect.prepare([layer(1, 1, smoothing=0, saturation=2)])
    # twice the distance from the luma, clipped
    assert drawn(effect)[0, 0].tolist() == pytest.approx(
        [min(255, 2 * 200 - luma), 2 * 100 - luma, 0.0]
    )


def test_smoothing_moves_towards_the_picture() -> None:
    screen, effect = make()
    effect.prepare([layer(1, 1, smoothing=100)])
    screen.push(sf.screen_frame(sf.solid(2, 2, (0, 0, 0))))
    assert drawn(effect, 0.01)[0, 0].tolist() == [0, 0, 0]  # the first picture at once
    screen.push(sf.screen_frame(sf.solid(2, 2, (200, 100, 50))))
    share = 1.0 - math.exp(-0.1 / 0.1)
    assert drawn(effect, 0.1)[0, 0].tolist() == pytest.approx(
        [200 * share, 100 * share, 50 * share]
    )
    again = drawn(effect, 0.1)[0, 0, 0]
    assert again == pytest.approx(200 * share + (200 - 200 * share) * share)


def test_no_smoothing_shows_each_frame_as_it_is() -> None:
    screen, effect = make()
    effect.prepare([layer(1, 1, smoothing=0)])
    screen.push(sf.screen_frame(sf.solid(2, 2, (0, 0, 0))))
    drawn(effect)
    screen.push(sf.screen_frame(sf.solid(2, 2, (200, 100, 50))))
    assert drawn(effect, 0.001)[0, 0].tolist() == [200, 100, 50]


def test_a_region_that_changes_size_starts_over() -> None:
    screen, effect = make()
    effect.prepare([layer(1, 1, smoothing=2000)])
    screen.push(sf.screen_frame(sf.solid(2, 2, (0, 0, 0))))
    drawn(effect)
    screen.push(sf.screen_frame(sf.solid(2, 2, (100, 100, 100))))
    assert drawn(effect, 0.01)[0, 0, 0] < 1.0  # slow
    effect.prepare([layer(2, 2, smoothing=2000)])
    assert drawn(effect, 0.01).tolist() == [[[100] * 3] * 2] * 2  # at once


def test_a_layer_keeps_its_smoothing_across_a_new_configuration() -> None:
    screen, effect = make()
    effect.prepare([layer(1, 1, smoothing=2000)])
    screen.push(sf.screen_frame(sf.solid(2, 2, (0, 0, 0))))
    drawn(effect)
    screen.push(sf.screen_frame(sf.solid(2, 2, (100, 100, 100))))
    effect.prepare([layer(1, 1, smoothing=2000)])
    assert drawn(effect, 0.01)[0, 0, 0] < 1.0


def test_layers_work_independently() -> None:
    screen, effect = make()
    screen.push(sf.screen_frame(picture([RED, BLUE])))
    effect.prepare([layer(1, 2, layer_id='a', smoothing=0), layer(1, 1, layer_id='b', smoothing=0)])
    first, second = effect.draw(0.01)
    assert first is not None and second is not None
    assert first.tolist() == [[list(RED), list(BLUE)]]
    assert second[0, 0].tolist() == pytest.approx([127.5, 0, 127.5])


# ---------------------------------------------------------------- on a surface


def surface_of(kind: str, width: int, height: int) -> dict[str, Any]:
    return {'kind': kind, 'resolution': {'width': width, 'height': height}, 'background': '#000000'}


def screen_layer(region: dict[str, Any], **params: Any) -> dict[str, Any]:
    return {
        'id': 's1',
        'effect': {'id': 'screen'},
        'params': {'smoothing': 0, **params},
        'region': region,
    }


def test_the_picture_fills_the_region_of_a_canvas() -> None:
    screen = ScreenInput('display-1', enabled=True)
    screen.push(sf.screen_frame(picture([RED, BLUE], [GREEN, YELLOW])))
    renderer = SurfaceRenderer(Analyzer(), inputs=EffectInputs(screen=screen))
    region = {'kind': 'rect', 'x': 0.5, 'y': 0.5, 'width': 0.5, 'height': 0.5}
    canvas = surface_of('canvas', 4, 4)
    layers = [screen_layer(region)]
    out, levels = renderer.compose(canvas, layers, Spectrum.silent(), 0.01)
    assert levels == {}
    block = out[2:, 2:]
    assert block[0, 0].tolist() == [255, 0, 0] and block[0, 1].tolist() == [0, 0, 255]
    assert block[1, 0].tolist() == [0, 255, 0] and block[1, 1].tolist() == [255, 255, 0]
    assert not out[:2].any() and not out[:, :2].any()  # the rest is the background


def test_a_layer_blends_over_what_is_below() -> None:
    screen = ScreenInput('display-1', enabled=True)
    screen.push(sf.screen_frame(sf.solid(2, 2, (100, 0, 0))))
    renderer = SurfaceRenderer(Analyzer(), inputs=EffectInputs(screen=screen))
    canvas = {**surface_of('canvas', 2, 2), 'background': '#0A0A0A'}
    layers = [{**screen_layer({'kind': 'relative'}), 'blend': 'add'}]
    out, _ = renderer.compose(canvas, layers, Spectrum.silent(), 0.01)
    assert out[0, 0].tolist() == [110, 10, 10]


def test_a_strip_draws_nothing() -> None:
    screen = ScreenInput('display-1', enabled=True)
    screen.push(sf.screen_frame(sf.solid(2, 2, (100, 0, 0))))
    renderer = SurfaceRenderer(Analyzer(), inputs=EffectInputs(screen=screen))
    out, _ = renderer.compose(
        surface_of('strip', 6, 1), [screen_layer({'kind': 'relative'})], Spectrum.silent(), 0.01
    )
    assert not out.any()


def test_without_a_screen_input_a_layer_draws_nothing() -> None:
    renderer = SurfaceRenderer(Analyzer())
    out, _ = renderer.compose(
        surface_of('canvas', 2, 2), [screen_layer({'kind': 'relative'})], Spectrum.silent(), 0.01
    )
    assert not out.any()


def test_sampling_a_large_frame_fits_the_frame_budget() -> None:
    """256 x 144 into 128 x 72 is the largest ADR-0037 D4 asks for; 50 Hz leaves 20 ms."""
    big = np.random.default_rng(1).random((144, 256, 3)) * 255.0
    best = min(_timed(lambda: sample_area(big, 72, 128)) for _ in range(5))
    assert best < 0.02, f'{best * 1000:.1f} ms'
    best = min(_timed(lambda: sample_edge(big, 72, 128, 0.15)) for _ in range(5))
    assert best < 0.02, f'{best * 1000:.1f} ms'


def _timed(work: Any) -> float:
    started = time.perf_counter()
    work()
    return time.perf_counter() - started


def test_area_sampling_matches_the_published_resampling() -> None:
    grid = np.random.default_rng(2).random((9, 14, 3)) * 255.0
    assert np.allclose(
        sample_area(grid, 4, 5),
        resample(grid.astype(np.float32), 5, 4).astype(np.float64),
        atol=1e-3,
    )
    weighted = np.einsum('hr,rcx->hcx', area_weights(9, 4), grid)
    weighted = np.einsum('wc,hcx->hwx', area_weights(14, 5), weighted)
    assert np.allclose(sample_area(grid, 4, 5), weighted, atol=1e-9)
    assert np.allclose(
        sample_area(grid, 20, 30),
        np.einsum(
            'hr,rcx->hcx', area_weights(9, 20), np.einsum('wc,rcx->rwx', area_weights(14, 30), grid)
        ),
        atol=1e-9,
    )


def test_missing_margins_are_none() -> None:
    assert Settings.of({'margins': None}).margins == (0.0, 0.0, 0.0, 0.0)
    assert Settings.of({}).margins == (0.0, 0.0, 0.0, 0.0)


def test_frames_compare_by_identity() -> None:
    first = sf.screen_frame(sf.solid(2, 2, (1, 2, 3)))
    same = sf.screen_frame(sf.solid(2, 2, (1, 2, 3)))
    assert first == first and first != same  # no array comparison
    assert len({first, same}) == 2
