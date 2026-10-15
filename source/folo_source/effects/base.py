"""
The effect interface (RB-04 step 4): what an effect declares (EffectInfo, as
the client API lists effects) and how a surface hands it its layers. An
effect instance renders all its layers of one surface, so it can work out
what they share once (the audio visualizer reads every band in one go).
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Protocol

import numpy as np
import numpy.typing as npt

from folo_source.inputs.audio.analyzer import Analyzer, Spectrum

# A layer's colours along its line: cells x RGB, on the 0-255 scale
Line = npt.NDArray[np.float64]
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


@dataclass(frozen=True, slots=True)
class LayerSpec:
    """A layer as its effect renders it: its parameters and the line it draws."""

    id: str
    params: Mapping[str, Any]
    length: int  # cells in its line: its region on a strip, its axis on a canvas
    visible: bool  # enabled, not hidden by a solo layer, and its region isn't empty


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


EffectFactory = Callable[[Analyzer, np.random.Generator], Effect]
_registry: dict[str, EffectFactory] = {}


def register(effect_id: str, factory: EffectFactory) -> None:
    """Makes an effect available under its id."""
    _registry[effect_id] = factory


def create(effect_id: str, analyzer: Analyzer, rng: np.random.Generator) -> Effect | None:
    """An instance of the effect `effect_id`, or None if there is no such effect."""
    factory = _registry.get(effect_id)
    return factory(analyzer, rng) if factory is not None else None


def effect_ids() -> list[str]:
    """The registered effects."""
    return sorted(_registry)
