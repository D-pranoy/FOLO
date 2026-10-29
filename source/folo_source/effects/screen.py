"""
The Screen effect, as far as the contract needs it today (RB-14 step 7): it
is declared and registered so the effect catalog in specs/ and the registry
agree, but it draws nothing until the capture agent's frames reach it
(docs/integrations/display-capture.md#the-screen-effect).
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

from folo_source.contracts import clamp
from folo_source.effects.base import (
    EffectInfo,
    LayerSpec,
    Levels,
    Line,
    register,
)
from folo_source.inputs.audio.analyzer import Analyzer, Spectrum

EFFECT_ID = 'screen'


def _schema() -> Mapping[str, Any]:
    try:
        return clamp.definition('ScreenParams')
    except OSError:  # a build without the bundled schemas
        return {}


class Screen:
    """The effect instance of one surface; no frames yet, so no layer draws a line."""

    INFO = EffectInfo(
        id=EFFECT_ID,
        version='1.0.0',
        name='Screen',
        description='The picture of a captured screen, without its black bars, as a canvas.',
        inputs=('screen',),
        params_schema=_schema(),
        min_elements=1,
    )

    def __init__(self, analyzer: Analyzer | None = None, rng: np.random.Generator | None = None):
        self.layers: list[LayerSpec] = []

    @property
    def info(self) -> EffectInfo:
        return self.INFO

    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        self.layers = list(layers)

    def render(
        self, spectrum: Spectrum, dt: float, beat: Any = None
    ) -> tuple[list[Line | None], Levels]:
        return [None] * len(self.layers), {}


register(EFFECT_ID, Screen)
