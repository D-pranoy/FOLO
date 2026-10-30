"""
The post-processing effects (RB-04 step 6): brightness and saturation,
mirror, blur and trail. Each changes what is below its layers in the stack,
inside their regions (docs/architecture/pipeline.md#effects); their
parameters are in docs/specs/config-model.md.
"""

from __future__ import annotations

import math
from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

from folo_source.contracts import clamp
from folo_source.effects.base import (
    EffectInfo,
    EffectInputs,
    LayerSpec,
    PostEffect,
    Values,
    register,
)
from folo_source.inputs.audio.analyzer import Analyzer

VERSION = '1.0.0'
# Rec. 709 luma weights
LUMA = np.array([0.2126, 0.7152, 0.0722])


def _schema(name: str) -> Mapping[str, Any]:
    try:
        return clamp.definition(name)
    except OSError:  # a build without the bundled schemas
        return {}


def _info(effect_id: str, name: str, description: str, params: str) -> EffectInfo:
    return EffectInfo(
        id=effect_id,
        version=VERSION,
        name=name,
        description=description,
        inputs=(),
        params_schema=_schema(params),
        min_elements=1,
    )


def _along(block: Values, axis: str) -> int:
    """The array axis a 1D operation runs along: a strip's cells, or a canvas's columns or rows."""
    return 0 if block.ndim == 2 or axis == 'linear_y' else 1


class _Post(PostEffect):
    """What every post-processing effect keeps: its layers' parameters and axes."""

    INFO: EffectInfo

    def __init__(
        self,
        analyzer: Analyzer | None = None,
        rng: np.random.Generator | None = None,
        inputs: EffectInputs | None = None,
    ):
        self.layers: list[LayerSpec] = []

    @property
    def info(self) -> EffectInfo:
        return self.INFO

    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        self.layers = list(layers)


class Brightness(_Post):
    INFO = _info(
        'brightness',
        'Brightness',
        'Scales the colours below it, and their saturation around their luma.',
        'BrightnessParams',
    )

    def apply(self, position: int, block: Values, dt: float) -> None:
        params = self.layers[position].params
        saturation = float(params.get('saturation', 1.0))
        brightness = float(params.get('brightness', 1.0))
        if saturation != 1.0:
            luma = (block @ LUMA)[..., None]
            block -= luma
            block *= saturation
            block += luma
        if brightness != 1.0:
            block *= brightness
        np.maximum(block, 0.0, out=block)
        np.minimum(block, 255.0, out=block)


class Mirror(_Post):
    INFO = _info(
        'mirror',
        'Mirror',
        'Copies one half of what is below it, reversed, onto the other half.',
        'MirrorParams',
    )

    def apply(self, position: int, block: Values, dt: float) -> None:
        layer = self.layers[position]
        along = _along(block, layer.axis)
        view = block if along == 0 else np.swapaxes(block, 0, 1)
        half = len(view) // 2
        if not half:
            return
        if layer.params.get('keep', 'first') == 'second':
            view[:half] = view[len(view) - half :][::-1].copy()
        else:
            view[len(view) - half :] = view[:half][::-1].copy()


def _box(block: Values, radius: int, along: int) -> Values:
    """The average of the cells up to `radius` away along one axis; the edge cells repeat."""
    pad = [(0, 0)] * block.ndim
    pad[along] = (radius + 1, radius)
    summed = np.cumsum(np.pad(block, pad, mode='edge'), axis=along)
    size = block.shape[along]
    window = 2 * radius + 1
    ahead: list[slice] = [slice(None)] * block.ndim
    behind: list[slice] = [slice(None)] * block.ndim
    ahead[along] = slice(window, window + size)
    behind[along] = slice(0, size)
    return (summed[tuple(ahead)] - summed[tuple(behind)]) / window


class Blur(_Post):
    INFO = _info(
        'blur',
        'Blur',
        'Averages each cell of what is below it with its neighbours.',
        'BlurParams',
    )

    def apply(self, position: int, block: Values, dt: float) -> None:
        radius = int(self.layers[position].params.get('radius', 1))
        result = _box(block, radius, 0)
        if block.ndim == 3:
            result = _box(result, radius, 1)
        block[...] = result


class Trail(_Post):
    INFO = _info(
        'trail',
        'Trail',
        'An afterglow: what is below it fades out instead of going dark at once.',
        'TrailParams',
    )

    def __init__(
        self,
        analyzer: Analyzer | None = None,
        rng: np.random.Generator | None = None,
        inputs: EffectInputs | None = None,
    ):
        super().__init__(analyzer, rng, inputs)
        self.previous: dict[str, Values] = {}

    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        super().prepare(layers, colors)
        ids = {layer.id for layer in self.layers}
        self.previous = {key: value for key, value in self.previous.items() if key in ids}

    def apply(self, position: int, block: Values, dt: float) -> None:
        layer = self.layers[position]
        previous = self.previous.get(layer.id)
        if previous is not None and previous.shape == block.shape:
            fade = float(layer.params.get('fade', 300)) / 1000.0
            np.maximum(block, previous * math.exp(-dt / fade), out=block)
        self.previous[layer.id] = block.copy()


for _effect in (Brightness, Mirror, Blur, Trail):
    register(_effect.INFO.id, _effect)
