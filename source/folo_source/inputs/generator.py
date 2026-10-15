"""
The generator input (RB-04 step 2): time, LFOs and noise for idle and
ambient effects. It needs no device, so it's always available and runs no
thread. Every value is a function of the time asked for, so effects get the
same values for the same frame.
"""

from __future__ import annotations

import math
import threading
from typing import Any, Literal

from folo_source.core import now_us
from folo_source.inputs.base import InputKind, InputState

LfoShape = Literal['sine', 'triangle', 'saw', 'square']

_MASK = (1 << 64) - 1


def _mix(value: int) -> int:
    """SplitMix64's finaliser: spreads the bits of a 64-bit value."""
    value = (value + 0x9E3779B97F4A7C15) & _MASK
    value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & _MASK
    value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & _MASK
    return value ^ (value >> 31)


class GeneratorInput:
    """`seed` makes the noise repeatable; time counts from `start_us`."""

    def __init__(self, seed: int = 0, start_us: int | None = None) -> None:
        self.seed = seed & _MASK
        self.start_us = now_us() if start_us is None else start_us

    @property
    def kind(self) -> InputKind:
        return 'generator'

    @property
    def thread(self) -> threading.Thread | None:
        return None

    def start(self) -> None:
        pass

    def stop(self) -> None:
        pass

    def health(self) -> InputState:
        return 'ok'

    def status(self) -> dict[str, Any]:
        return {'state': 'ok'}

    def time(self, at_us: int) -> float:
        """Seconds since the input started."""
        return (at_us - self.start_us) / 1e6

    def lfo(
        self, at_us: int, rate_hz: float, shape: LfoShape = 'sine', phase: float = 0.0
    ) -> float:
        """
        A low-frequency oscillator, 0-1, at `rate_hz` cycles a second.
        `phase` (0-1) shifts it; every shape starts a cycle at 0.
        """
        position = (self.time(at_us) * rate_hz + phase) % 1.0
        if shape == 'sine':
            return 0.5 - 0.5 * math.cos(2.0 * math.pi * position)
        if shape == 'triangle':
            return 1.0 - abs(2.0 * position - 1.0)
        if shape == 'saw':
            return position
        return 0.0 if position < 0.5 else 1.0

    def noise(self, at_us: int, rate_hz: float, channel: int = 0) -> float:
        """
        Smooth noise, 0-1: a new random value `rate_hz` times a second,
        eased from one to the next. Channels are independent of each other.
        """
        position = self.time(at_us) * rate_hz
        step = math.floor(position)
        t = position - step
        eased = t * t * (3.0 - 2.0 * t)
        here = self._random(channel, step)
        return here + (self._random(channel, step + 1) - here) * eased

    def _random(self, channel: int, step: int) -> float:
        key = _mix(self.seed ^ _mix((channel & 0xFFFFFFFF) << 32 ^ (step & _MASK)))
        return (key >> 11) / float(1 << 53)
