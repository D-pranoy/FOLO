"""
The publisher (RB-04 step 7, docs/architecture/pipeline.md#publishing):
turns rendered frames and signals into source-link frames as `config.apply`'s
`publish` asks: each surface at its rate, resolution and pixel format, and
the signals it names. It does no I/O; the link sends what it returns.
"""

from __future__ import annotations

import logging
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any, cast

from folo_source.core import Frame, PixelFormat, Signal
from folo_source.publish import frames

log = logging.getLogger(__name__)

US_PER_S = 1_000_000


@dataclass(slots=True)
class _Route:
    """How one surface is published."""

    width: int
    height: int
    period_us: float
    pixel_format: PixelFormat
    lz4: bool
    due_us: float | None = None  # when the next send falls due


class Publisher:
    """
    Publishes the surfaces and signals of the latest `configure()`. Sequence
    numbers count the frames sent per surface and per signal index from the
    publisher's start, so they keep counting across configurations and
    reconnects.
    """

    def __init__(self) -> None:
        self._routes: dict[int, _Route] = {}
        self._signals: dict[str, int] = {}  # name -> signal index
        self._surface_sequences: dict[int, int] = {}
        self._signal_sequences: dict[int, int] = {}
        self._unknown: set[str] = set()
        self.sent = 0  # surface frames, for metrics

    def configure(self, publish: Mapping[str, Any]) -> None:
        """Takes `config.apply`'s `publish` (source-link.json's Publish)."""
        routes: dict[int, _Route] = {}
        for entry in cast(Sequence[Mapping[str, Any]], publish.get('surfaces', [])):
            index = int(entry['index'])
            old = self._routes.get(index)
            route = _Route(
                int(entry['width']),
                int(entry['height']),
                US_PER_S / float(entry['rate_hz']),
                cast(PixelFormat, entry['pixel_format']),
                bool(entry['lz4']),
            )
            if old is not None:
                route.due_us = old.due_us  # the rate keeps its count
            routes[index] = route
        self._routes = routes
        self._signals = {
            str(entry['name']): int(entry['index'])
            for entry in cast(Sequence[Mapping[str, Any]], publish.get('signals', []))
        }

    @property
    def surfaces(self) -> tuple[int, ...]:
        """The surface indices published."""
        return tuple(self._routes)

    def wants(self, name: str) -> bool:
        """Whether the signal `name` is published."""
        return name in self._signals

    def surface(self, index: int, frame: Frame) -> bytes | None:
        """
        The surface frame to send for a newly rendered frame of surface
        `index`, or None when it isn't published or its next send isn't due.
        """
        route = self._routes.get(index)
        if route is None or not self._due(route, frame.timestamp_us):
            return None
        pixels = frames.resample(frame.pixels, route.width, route.height)
        sequence = self._surface_sequences.get(index, 0)
        self._surface_sequences[index] = sequence + 1
        self.sent += 1
        return frames.surface_frame(
            index, pixels, route.pixel_format, sequence, frame.timestamp_us, route.lz4
        )

    def signal(self, signal: Signal) -> bytes | None:
        """The signal frame to send for a signal, or None when it isn't published."""
        index = self._signals.get(signal.name)
        if index is None:
            return None
        sequence = self._signal_sequences.get(index, 0)
        self._signal_sequences[index] = sequence + 1
        return frames.signal_frame(index, signal.values, sequence, signal.timestamp_us)

    def unknown_signals(self, known: Sequence[str]) -> list[str]:
        """
        The signals asked for that this source doesn't have, each logged
        once: they are never sent.
        """
        missing = [name for name in self._signals if name not in known]
        for name in missing:
            if name not in self._unknown:
                self._unknown.add(name)
                log.warning('asked to publish the signal %r, which this source does not have', name)
        return missing

    @staticmethod
    def _due(route: _Route, now_us: int) -> bool:
        """
        Whether a frame rendered at `now_us` is sent: sends fall due every
        period from the first, and the count starts again after a pause
        longer than one period, so a slower surface sends every frame.
        """
        due = route.due_us
        if due is None or now_us - due > route.period_us:
            due = float(now_us)
        if now_us < due:
            return False
        route.due_us = due + route.period_us
        return True
