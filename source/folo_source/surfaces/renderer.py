"""
Surfaces (RB-04 steps 5 and 6): a strip or a canvas with its layer stack.
Each layer's effect draws a line; the line fills the layer's region, along
the layer's axis on a canvas. Post-processing layers change what is below
them instead. The stack is worked bottom first over the background, then
`master` scales the result and it's clipped to 0-255
(docs/architecture/pipeline.md#surfaces-and-layer-stacks).
"""

from __future__ import annotations

import math
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np
import numpy.typing as npt

from folo_source.core import Frame, to_rgbf32
from folo_source.effects import base
from folo_source.effects import post as _post  # registers the post-processing effects
from folo_source.effects import screen as _screen  # registers the screen effect
from folo_source.effects import visualizers as _visualizers  # registers the audio visualizer
from folo_source.effects.base import Effect, LayerSpec, Levels, Line, PostEffect
from folo_source.effects.colors import hex_to_rgb
from folo_source.inputs.audio.analyzer import Analyzer, Spectrum

Values = npt.NDArray[np.float64]
Indices = npt.NDArray[np.intp]

_ = (_post, _screen, _visualizers)


def strip_cells(region: Mapping[str, Any], size: int) -> tuple[int, int]:
    """
    The cells [first, stop) a region covers on a strip of `size` cells,
    clipped to it; stop <= first when it covers none.
    """
    kind = region.get('kind', 'relative')
    if kind == 'cells':
        first, last = int(region['start']), int(region['end'])
    else:
        if kind == 'rect':
            lo = float(region.get('x', 0.0))
            hi = lo + float(region.get('width', 1.0))
        else:
            lo, hi = float(region.get('start', 0.0)), float(region.get('end', 1.0))
        first = math.floor(lo * size)
        last = max(first, math.ceil(hi * size) - 1)
    return max(0, first), min(last, size - 1) + 1


def canvas_cells(region: Mapping[str, Any], width: int, height: int) -> tuple[int, int, int, int]:
    """The rows [r0, r1) and columns [c0, c1) a region covers on a canvas."""
    if region.get('kind') == 'rect':
        x, y = float(region.get('x', 0.0)), float(region.get('y', 0.0))
        c0 = math.floor(x * width)
        c1 = max(c0, math.ceil((x + float(region.get('width', 1.0))) * width) - 1) + 1
        r0 = math.floor(y * height)
        r1 = max(r0, math.ceil((y + float(region.get('height', 1.0))) * height) - 1) + 1
        return max(0, r0), min(r1, height), max(0, c0), min(c1, width)
    c0, c1 = strip_cells(region, width)
    return 0, height, c0, c1


def axis_line(axis: str, rows: int, cols: int) -> tuple[int, Indices]:
    """
    How a 1D effect fills a rows x cols block: the length of its line, and
    for every cell of the block the cell of the line it takes.
    """
    y = (np.arange(rows, dtype=np.float64) + 0.5)[:, None]
    x = (np.arange(cols, dtype=np.float64) + 0.5)[None, :]
    if axis == 'linear_y':
        length, t = rows, np.broadcast_to(y / rows, (rows, cols))
    elif axis == 'radial':
        length = max(1, round(math.hypot(rows, cols) / 2.0))
        dy, dx = y - rows / 2.0, x - cols / 2.0
        t = np.hypot(dy, dx) / math.hypot(rows / 2.0, cols / 2.0)
    elif axis == 'angular':
        length = max(1, 2 * (rows + cols))
        dy, dx = y - rows / 2.0, x - cols / 2.0
        # Clockwise from the side of row 0 (up), as a share of a turn
        t = np.mod(np.arctan2(dx, -dy), 2.0 * math.pi) / (2.0 * math.pi)
    else:  # linear_x
        length, t = cols, np.broadcast_to(x / cols, (rows, cols))
    index = np.minimum(length - 1, np.floor(t * length).astype(np.intp))
    return length, index


def blend(mode: str, below: Values, layer: npt.NDArray[np.floating]) -> None:
    """Blends a layer into what is below it, in place, on the 0-255 scale."""
    if mode == 'add':
        np.add(below, layer, out=below)
        np.minimum(below, 255.0, out=below)
    elif mode == 'multiply':
        np.multiply(below, layer, out=below)
        below /= 255.0
    elif mode == 'alpha':
        opacity = layer.max(axis=-1, keepdims=True) / 255.0
        below *= 1.0 - opacity
        below += layer
    else:  # max, today's behaviour
        np.maximum(below, layer, out=below)


@dataclass(slots=True)
class _Entry:
    """A layer of the stack, as worked out once per configuration."""

    effect_id: str
    position: int  # among its effect's layers
    blend: str
    visible: bool
    cols: slice
    rows: slice | None = None  # canvases only
    index: Indices | None = None  # canvases: the line cell of every cell
    post: bool = False  # a post-processing layer: it changes what is below it


class SurfaceRenderer:
    """
    Renders one surface: its layers' effects (one instance per effect, for
    all its layers here) and their blending. The configuration is worked out
    again when the surface or layers object, or the album colours, change:
    callers replace them as a whole, never edit them.
    """

    def __init__(self, analyzer: Analyzer, rng: np.random.Generator | None = None) -> None:
        self.analyzer = analyzer
        self._rng = np.random.default_rng() if rng is None else rng
        self.effects: dict[str, Effect | PostEffect] = {}
        self._surface: object = None
        self._layers: object = None
        self._colors: tuple[str, ...] = ()
        self._base: Values = np.zeros((0, 3))
        self._entries: list[_Entry] = []
        # Seconds each effect took in the last compose(), all its layers together
        self.timings: dict[str, float] = {}

    @property
    def rng(self) -> np.random.Generator:
        """The random numbers of the random modes (drops, fire)."""
        return self._rng

    @rng.setter
    def rng(self, rng: np.random.Generator) -> None:
        self._rng = rng
        self.effects.clear()  # made again, with it, on the next frame
        self._surface = self._layers = None

    def prepare(
        self,
        surface: Mapping[str, Any],
        layers: Sequence[Mapping[str, Any]],
        colors: tuple[str, ...] = (),
    ) -> None:
        """Works out the stack for a surface and its layers (zones.json, show.json)."""
        canvas = surface.get('kind') == 'canvas'
        resolution = surface.get('resolution', {})
        width = int(resolution.get('width', 1))
        height = int(resolution.get('height', 1)) if canvas else 1
        background = np.array(hex_to_rgb(surface.get('background', '#000000')), dtype=np.float64)
        self._base = np.tile(background, (height, width, 1) if canvas else (width, 1))
        solo = any(layer.get('solo', False) and layer.get('enabled', True) for layer in layers)
        grouped: dict[str, list[LayerSpec]] = {}
        entries: list[_Entry] = []
        for layer in layers:
            effect_id = str(layer.get('effect', {}).get('id', ''))
            region = layer.get('region', {})
            shown = bool(layer.get('enabled', True)) and (
                bool(layer.get('solo', False)) or not solo
            )
            if canvas:
                r0, r1, c0, c1 = canvas_cells(region, width, height)
                empty = r1 <= r0 or c1 <= c0
                axis = str(layer.get('axis', 'linear_x'))
                length, index = (0, None) if empty else axis_line(axis, r1 - r0, c1 - c0)
                rows: slice | None = slice(r0, r1)
            else:
                c0, c1 = strip_cells(region, width)
                empty = c1 <= c0
                length, index, rows = c1 - c0, None, None
            specs = grouped.setdefault(effect_id, [])
            visible = shown and not empty
            entries.append(
                _Entry(
                    effect_id,
                    len(specs),
                    str(layer.get('blend', 'max')),
                    visible,
                    slice(c0, c1),
                    rows,
                    index,
                )
            )
            spec = LayerSpec(
                str(layer.get('id', '')),
                layer.get('params', {}),
                max(0, length),
                visible,
                str(layer.get('axis', 'linear_x')),
            )
            specs.append(spec)
        effects: dict[str, Effect | PostEffect] = {}
        for effect_id, specs in grouped.items():
            effect = self.effects.get(effect_id) or base.create(effect_id, self.analyzer, self._rng)
            if effect is None:  # an effect this engine doesn't have draws nothing
                continue
            effect.prepare(specs, colors)
            effects[effect_id] = effect
        for entry in entries:
            entry.visible = entry.visible and entry.effect_id in effects
            entry.post = isinstance(effects.get(entry.effect_id), PostEffect)
        self.effects = effects
        self._entries = entries
        self._surface, self._layers, self._colors = surface, layers, colors

    def compose(
        self,
        surface: Mapping[str, Any],
        layers: Sequence[Mapping[str, Any]],
        spectrum: Spectrum,
        dt: float,
        beat: Any = None,
        colors: tuple[str, ...] = (),
    ) -> tuple[Values, Levels]:
        """
        The stack blended over the background, before `master`, and every
        layer's levels. `beat`: the zone's beat tracker; `colors`: the album
        art's colours, when the zone uses them.
        """
        if surface is not self._surface or layers is not self._layers or colors != self._colors:
            self.prepare(surface, layers, colors)
        out = self._base.copy()
        levels: Levels = {}
        lines: dict[str, list[Line | None]] = {}
        timings = dict.fromkeys(self.effects, 0.0)
        for effect_id, effect in self.effects.items():
            if isinstance(effect, PostEffect):
                continue
            started = time.perf_counter()
            lines[effect_id], effect_levels = effect.render(spectrum, dt, beat)
            timings[effect_id] = time.perf_counter() - started
            levels.update(effect_levels)
        for entry in self._entries:
            if not entry.visible:
                continue
            if entry.post:
                effect = self.effects[entry.effect_id]
                assert isinstance(effect, PostEffect)
                below = out[entry.cols] if entry.rows is None else out[entry.rows, entry.cols]
                started = time.perf_counter()
                effect.apply(entry.position, below, dt)
                timings[entry.effect_id] += time.perf_counter() - started
                continue
            line = lines[entry.effect_id][entry.position]
            if line is None:
                continue
            if entry.index is None:
                blend(entry.blend, out[entry.cols], line)
            else:
                blend(entry.blend, out[entry.rows, entry.cols], line[entry.index])
        self.timings = timings
        return out, levels

    @staticmethod
    def finish(out: Values, master: float) -> Values:
        """Scales the stack by `master` and clips it to 0-255, in place."""
        if master != 1.0:
            out *= master
        # np.clip's own overhead is twice that of these two
        np.maximum(out, 0.0, out=out)
        np.minimum(out, 255.0, out=out)
        return out

    def render(
        self,
        surface: Mapping[str, Any],
        layers: Sequence[Mapping[str, Any]],
        spectrum: Spectrum,
        dt: float,
        timestamp_us: int,
        sequence: int,
        beat: Any = None,
        colors: tuple[str, ...] = (),
    ) -> tuple[Frame, Levels]:
        """One frame of the surface, as rgbf32, and its layers' levels."""
        out, levels = self.compose(surface, layers, spectrum, dt, beat, colors)
        self.finish(out, float(surface.get('master', 1.0)))
        return Frame(to_rgbf32(out), timestamp_us, sequence), levels
