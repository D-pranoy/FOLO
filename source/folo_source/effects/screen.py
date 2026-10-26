"""
The Screen effect (RB-14 step 7, ADR-0037 D8,
docs/integrations/display-capture.md#the-screen-effect): a canvas effect
that turns the newest usable frame of the screen input into a picture for
each layer's rectangle. Per frame: crop the black bars and margins, sample
by area or from the picture's edges, colour, then smooth towards it.
"""

from __future__ import annotations

import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np

from folo_source.contracts import clamp
from folo_source.effects.base import (
    CanvasEffect,
    EffectInfo,
    EffectInputs,
    LayerSpec,
    Values,
    register,
)
from folo_source.effects.post import LUMA
from folo_source.inputs.audio.analyzer import Analyzer
from folo_source.inputs.screen import ScreenPicture
from folo_source.publish.frames import ScreenFrame, area_weights

EFFECT_ID = 'screen'


def _schema() -> Mapping[str, Any]:
    try:
        return clamp.definition('ScreenParams')
    except OSError:  # a build without the bundled schemas
        return {}


@dataclass(frozen=True, slots=True)
class Settings:
    """A layer's parameters (ScreenParams), with the defaults of ADR-0037 D8."""

    mode: str = 'area'
    edge_depth: float = 0.15
    margins: tuple[float, float, float, float] = (0.0, 0.0, 0.0, 0.0)  # top, bottom, left, right
    brightness: float = 1.0
    saturation: float = 1.0
    smoothing: float = 150.0  # ms
    bars: str = 'follow'

    @staticmethod
    def of(params: Mapping[str, Any]) -> Settings:
        margins: Mapping[str, Any] = params.get('margins', {})
        return Settings(
            mode=str(params.get('mode', 'area')),
            edge_depth=float(params.get('edge_depth', 0.15)),
            margins=(
                float(margins.get('top', 0.0)),
                float(margins.get('bottom', 0.0)),
                float(margins.get('left', 0.0)),
                float(margins.get('right', 0.0)),
            ),
            brightness=float(params.get('brightness', 1.0)),
            saturation=float(params.get('saturation', 1.0)),
            smoothing=float(params.get('smoothing', 150.0)),
            bars=str(params.get('bars', 'follow')),
        )


def margin_cells(shares: tuple[float, float], size: int) -> tuple[int, int]:
    """
    The cells to leave out at the two ends of `size` cells, for two shares
    of them rounded to the nearest cell; at least one cell stays (the second
    end gives way first).
    """
    first, second = (int(share * size + 0.5) for share in shares)
    excess = first + second - (size - 1)
    if excess > 0:
        second = max(0, second - excess)
        first = min(first, size - 1 - second)
    return first, second


def crop(frame: ScreenFrame, pixels: Values, settings: Settings) -> Values:
    """
    The part of the frame to show: without the black bars the agent found
    (`bars` follow), then without the margins, shares of what remains.
    """
    top, bottom, left, right = frame.bars if settings.bars == 'follow' else (0, 0, 0, 0)
    height, width = pixels.shape[0] - top - bottom, pixels.shape[1] - left - right
    m_top, m_bottom = margin_cells(settings.margins[:2], height)
    m_left, m_right = margin_cells(settings.margins[2:], width)
    return pixels[
        top + m_top : pixels.shape[0] - bottom - m_bottom,
        left + m_left : pixels.shape[1] - right - m_right,
    ]


def sample_area(picture: Values, rows: int, cols: int) -> Values:
    """The picture on rows x cols cells, each the area average of the cells under it."""
    height, width = picture.shape[:2]
    across = np.einsum('wc,rcx->rwx', area_weights(width, cols), picture)
    return np.einsum('hr,rwx->hwx', area_weights(height, rows), across)


def band_weights(size: int, depth: float) -> Values:
    """Weights over the first `depth` share of `size` cells (a part cell in part), summing to 1."""
    reach = max(depth * size, 1e-9)
    cells = np.arange(size, dtype=np.float64)
    covered = np.clip(np.minimum(reach, cells + 1.0) - cells, 0.0, 1.0)
    return covered / covered.sum()


def sample_edge(picture: Values, rows: int, cols: int, depth: float) -> Values:
    """
    The picture on rows x cols cells, each the colour of the nearest side of
    the rectangle: the average of a band of the picture along that side,
    `depth` of its height or width deep, over the span `sample_area` gives
    the cell along the side.
    """
    height, width = picture.shape[:2]
    deep_rows, deep_cols = band_weights(height, depth), band_weights(width, depth)
    along_x, along_y = area_weights(width, cols), area_weights(height, rows)
    top = along_x @ np.tensordot(deep_rows, picture, axes=(0, 0))  # (cols, 3)
    bottom = along_x @ np.tensordot(deep_rows[::-1], picture, axes=(0, 0))
    left = along_y @ np.tensordot(deep_cols, picture, axes=(0, 1))  # (rows, 3)
    right = along_y @ np.tensordot(deep_cols[::-1], picture, axes=(0, 1))
    y = (np.arange(rows, dtype=np.float64) + 0.5)[:, None]
    x = (np.arange(cols, dtype=np.float64) + 0.5)[None, :]
    distances = np.stack(np.broadcast_arrays(y, rows - y, x, cols - x))
    nearest = np.argmin(distances, axis=0)[..., None]  # the first of equals wins
    return np.select(
        [nearest == 0, nearest == 1, nearest == 2],
        [top[None, :, :], bottom[None, :, :], left[:, None, :]],
        default=right[:, None, :],
    )


def colour(block: Values, settings: Settings) -> None:
    """Saturation around the luma, then brightness, clipped to 0-255, as `brightness` does."""
    if settings.saturation != 1.0:
        luma = (block @ LUMA)[..., None]
        block -= luma
        block *= settings.saturation
        block += luma
    if settings.brightness != 1.0:
        block *= settings.brightness
    np.maximum(block, 0.0, out=block)
    np.minimum(block, 255.0, out=block)


@dataclass(slots=True)
class _Layer:
    """What a layer keeps: its latest target, and the picture it shows."""

    frame: ScreenFrame
    settings: Settings
    target: Values
    shown: Values


class Screen(CanvasEffect):
    """The Screen layers of one surface; they all read the engine's screen input."""

    INFO = EffectInfo(
        id=EFFECT_ID,
        version='1.0.0',
        name='Screen',
        description='The picture of a captured screen, without its black bars, as a canvas.',
        inputs=('screen',),
        params_schema=_schema(),
        min_elements=1,
    )

    def __init__(
        self,
        analyzer: Analyzer | None = None,
        rng: np.random.Generator | None = None,
        inputs: EffectInputs | None = None,
    ):
        self.screen = inputs.screen if inputs is not None else None
        self.layers: list[LayerSpec] = []
        self._kept: dict[str, _Layer] = {}
        self._wide: tuple[ScreenFrame, Values] | None = None  # the frame as float64

    @property
    def info(self) -> EffectInfo:
        return self.INFO

    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        self.layers = list(layers)
        ids = {layer.id for layer in self.layers}
        self._kept = {key: value for key, value in self._kept.items() if key in ids}

    def draw(self, dt: float) -> list[Values | None]:
        picture = self.screen.picture() if self.screen is not None else None
        if picture is None:  # a layer that never had one, or the screen was turned off
            self._kept.clear()
            self._wide = None
            return [None] * len(self.layers)
        return [self._draw_layer(layer, picture, dt) for layer in self.layers]

    def _draw_layer(self, layer: LayerSpec, picture: ScreenPicture, dt: float) -> Values | None:
        if not layer.visible or layer.rows < 1 or layer.cols < 1:
            self._kept.pop(layer.id, None)
            return None
        settings = Settings.of(layer.params)
        kept = self._kept.get(layer.id)
        shape = (layer.rows, layer.cols, 3)
        if kept is None or kept.shown.shape != shape:  # a new layer, or its region changed size
            target = self._target(picture.frame, settings, layer)
            kept = self._kept[layer.id] = _Layer(picture.frame, settings, target, target.copy())
            return kept.shown
        if kept.frame is not picture.frame or kept.settings != settings:
            kept.target = self._target(picture.frame, settings, layer)
            kept.frame, kept.settings = picture.frame, settings
        if settings.smoothing > 0.0:
            kept.shown += (kept.target - kept.shown) * (
                1.0 - math.exp(-dt / (settings.smoothing / 1000.0))
            )
        else:
            kept.shown[...] = kept.target
        return kept.shown

    def _target(self, frame: ScreenFrame, settings: Settings, layer: LayerSpec) -> Values:
        """The layer's rectangle as the newest picture colours it, before smoothing."""
        if self._wide is None or self._wide[0] is not frame:
            self._wide = (frame, frame.pixels.astype(np.float64))
        shown = crop(frame, self._wide[1], settings)
        if settings.mode == 'edge':
            target = sample_edge(shown, layer.rows, layer.cols, settings.edge_depth)
        else:
            target = sample_area(shown, layer.rows, layer.cols)
        target = np.array(target, dtype=np.float64)  # a copy that can be coloured in place
        colour(target, settings)
        return target


register(EFFECT_ID, Screen)
