"""
The screen input (RB-04 step 2, RB-14 step 7, ADR-0037 D12 and D13): the
newest screen frame the capture agent's node forwarded, and when it arrived
on the engine's clock. A frame the agent holds or marks protected keeps its
state but not its pixels: the input keeps drawing the last picture without
either flag. Raw frames stay in this object, in memory (ADR-0037 D9).
"""

from __future__ import annotations

import threading
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from folo_source.core import now_us
from folo_source.inputs.base import InputKind, InputState
from folo_source.publish.frames import ScreenFrame

# A picture counts as live while the newest usable frame is younger than this (D12)
STALE_US = 500_000
RATE_WINDOW_US = 1_000_000  # the span the frame rate is counted over


@dataclass(frozen=True, slots=True)
class ScreenPicture:
    """The newest usable frame and the engine's time it arrived."""

    frame: ScreenFrame
    arrived_us: int


class ScreenInput:
    """
    The screen `screen_id` (sources.json's screen inputs). `push()` takes the
    frames from the link's thread, `picture()` serves the render thread.
    `clock` is the engine's microsecond clock; `on_frame` is called after a
    frame was taken (it wakes the renderer).
    """

    def __init__(
        self,
        screen_id: str,
        enabled: bool = False,
        clock: Callable[[], int] = now_us,
        on_frame: Callable[[], None] | None = None,
    ) -> None:
        self.screen_id = screen_id
        self.enabled = enabled
        self.clock = clock
        self.on_frame = on_frame
        self._lock = threading.Lock()
        self._picture: ScreenPicture | None = None
        self._newest: ScreenFrame | None = None  # whatever came last: its flags and bars
        self._arrivals: list[int] = []  # of the frames in the last RATE_WINDOW_US

    @property
    def kind(self) -> InputKind:
        return 'screen'

    @property
    def thread(self) -> threading.Thread | None:
        return None

    def start(self) -> None:
        pass

    def stop(self) -> None:
        pass

    def configure(self, screen_id: str, enabled: bool) -> None:
        """
        Takes the settings of the source's screen input. A screen that is
        turned off, or another one, forgets the picture it had.
        """
        with self._lock:
            if not enabled or screen_id != self.screen_id:
                self._forget()
            self.screen_id, self.enabled = screen_id, enabled

    def push(self, frame: ScreenFrame) -> None:
        """
        Takes a frame, stamping it with its arrival. The link delivers them
        in order, so the newest is the last one pushed. While the input is
        off it keeps nothing.
        """
        arrived = self.clock()
        # The frame's screen index isn't compared: one screen per source in v1
        with self._lock:
            if not self.enabled:
                return
            self._newest = frame
            self._arrivals = [t for t in self._arrivals if arrived - t < RATE_WINDOW_US]
            self._arrivals.append(arrived)
            if frame.usable:
                self._picture = ScreenPicture(frame, arrived)
        if self.on_frame is not None:
            self.on_frame()

    def picture(self) -> ScreenPicture | None:
        """The newest frame that is neither held nor protected, or none yet."""
        with self._lock:
            return self._picture

    def active(self, now: int) -> bool:
        """Whether the newest usable frame arrived less than STALE_US before `now`."""
        picture = self.picture()
        return picture is not None and now - picture.arrived_us < STALE_US

    def inactive_since(self, now: int, since: int) -> float:
        """
        The seconds since the input was last active, as of `now`; `since` is
        when to count from if it never was (the clock of the first look).
        """
        picture = self.picture()
        ended = since if picture is None else picture.arrived_us + STALE_US
        return max(0.0, (now - ended) / 1e6)

    def health(self) -> InputState:
        if not self.enabled:
            return 'idle'
        return 'ok' if self.active(self.clock()) else 'waiting'

    def status(self) -> dict[str, Any]:
        """
        What the input says about itself: state, rate, the size of the frame,
        flags and bars, never a pixel (ADR-0037 D9).
        """
        now = self.clock()
        with self._lock:
            newest = self._newest
            rate = sum(1 for t in self._arrivals if now - t < RATE_WINDOW_US)
        flags: dict[str, bool] = {}
        bars: tuple[int, int, int, int] = (0, 0, 0, 0)
        size = (0, 0)
        if newest is not None:
            flags = {'protected': newest.protected, 'hdr': newest.hdr, 'held': newest.held}
            bars = newest.bars
            size = (int(newest.pixels.shape[1]), int(newest.pixels.shape[0]))
        return {
            'id': self.screen_id,
            'enabled': self.enabled,
            'state': self.health(),
            'rate': rate,
            'width': size[0],
            'height': size[1],
            'bars': list(bars),
            **flags,
        }

    def _forget(self) -> None:
        self._picture = None
        self._newest = None
        self._arrivals = []
