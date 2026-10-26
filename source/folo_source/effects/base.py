"""
The effect interface (RB-04 steps 4 and 6): what an effect declares
(EffectInfo, as the client API lists effects) and how a surface hands it its
layers. An effect instance handles all its layers of one surface, so it can
work out what they share once (the audio visualizer reads every band in one
go). Most effects draw a line per layer; post-processing effects change what
is below them in the stack instead (PostEffect); canvas effects draw a 2D
picture for their layer's rectangle of a canvas (CanvasEffect).
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Protocol

import numpy as np
import numpy.typing as npt

from folo_source.inputs.audio.analyzer import Analyzer, Spectrum
from folo_source.inputs.screen import ScreenInput

# A layer's colours along its line: cells x RGB, on the 0-255 scale
Line = npt.NDArray[np.float64]
# Cells of a surface: cells x RGB (strip) or rows x columns x RGB (canvas)
Values = npt.NDArray[np.float64]
# {layer id: (level dB, value 0-1, recent average dB)}
Levels = dict[str, tuple[float, float, float]]


@dataclass(frozen=True, slots=True)
class EffectInfo:
    """What an effect declares (api.json's EffectInfo)."""

    id: str
    version: str
    name: str
    description: str
    inputs: tuple[str, ...]  # audio, beat, media, screen, generator
    params_schema: Mapping[str, Any]
    min_elements: int

    def as_dict(self) -> dict[str, Any]:
        return {
            'id': self.id,
            'version': self.version,
            'name': self.name,
            'description': self.description,
            'inputs': list(self.inputs),
            'params_schema': dict(self.params_schema),
            'min_elements': self.min_elements,
        }


def shown_levels(levels: Levels) -> dict[str, list[float]]:
    """
    Layer levels as clients get them: {layer id: [level dB, value, recent
    average dB]}, rounded.
    """
    return {
        layer_id: [round(level, 1), round(value, 3), round(average, 1)]
        for layer_id, (level, value, average) in levels.items()
    }


@dataclass(frozen=True, slots=True)
class LayerSpec:
    """A layer as its effect renders it: its parameters and the line it draws."""

    id: str
    params: Mapping[str, Any]
    length: int  # cells in its line: its region on a strip, its axis on a canvas
    visible: bool  # enabled, not hidden by a solo layer, and its region isn't empty
    axis: str = 'linear_x'  # on a canvas: linear_x, linear_y, radial or angular
    rows: int = 0  # cells of its rectangle on a canvas; 0 on a strip
    cols: int = 0


class Effect(Protocol):
    """An effect instance, rendering its layers of one surface."""

    @property
    def info(self) -> EffectInfo: ...

    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        """
        Takes its layers, in stack order, when the configuration changes.
        `colors`: the album art's colours, when the zone uses them.
        """
        ...

    def render(
        self, spectrum: Spectrum, dt: float, beat: Any = None
    ) -> tuple[list[Line | None], Levels]:
        """
        One frame: each layer's line (None when it isn't visible), in the
        order prepare() got them, and their levels. `beat`: the zone's beat
        tracker, pushed with the same audio.
        """
        ...


class PostEffect(ABC):
    """
    A post-processing effect instance: it changes what is below its layers
    in the stack, inside their regions, instead of drawing a line.
    """

    @property
    @abstractmethod
    def info(self) -> EffectInfo: ...

    @abstractmethod
    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        """Takes its layers, in stack order, when the configuration changes."""

    @abstractmethod
    def apply(self, position: int, block: Values, dt: float) -> None:
        """
        Changes, in place, the cells below its layer at `position` (as
        prepare() got them): cells x RGB on a strip, rows x columns x RGB on
        a canvas, on the 0-255 scale. `dt`: seconds since the last frame.
        """


class CanvasEffect(ABC):
    """
    A canvas effect instance: for each layer it draws a picture of the
    layer's rectangle (rows x columns), where an effect draws a line. On a
    strip surface its layers have no rectangle and draw nothing.
    """

    @property
    @abstractmethod
    def info(self) -> EffectInfo: ...

    @abstractmethod
    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        """Takes its layers, in stack order, when the configuration changes."""

    @abstractmethod
    def draw(self, dt: float) -> list[Values | None]:
        """
        One frame: each layer's picture, rows x columns x RGB on the 0-255
        scale (None when it isn't visible or has nothing to show), in the
        order prepare() got them. `dt`: seconds since the last frame.
        """


@dataclass(frozen=True, slots=True)
class EffectInputs:
    """
    The engine's inputs an effect may read besides the audio the analyzer
    holds (ADR-0037 D10). Effects that need none ignore it.
    """

    screen: ScreenInput | None = None


AnyEffect = Effect | PostEffect | CanvasEffect
EffectFactory = Callable[[Analyzer, np.random.Generator, EffectInputs], AnyEffect]
_registry: dict[str, EffectFactory] = {}


def register(effect_id: str, factory: EffectFactory) -> None:
    """Makes an effect available under its id."""
    _registry[effect_id] = factory


def create(
    effect_id: str,
    analyzer: Analyzer,
    rng: np.random.Generator,
    inputs: EffectInputs | None = None,
) -> AnyEffect | None:
    """An instance of the effect `effect_id`, or None if there is no such effect."""
    factory = _registry.get(effect_id)
    if factory is None:
        return None
    return factory(analyzer, rng, EffectInputs() if inputs is None else inputs)


def effect_ids() -> list[str]:
    """The registered effects."""
    return sorted(_registry)
