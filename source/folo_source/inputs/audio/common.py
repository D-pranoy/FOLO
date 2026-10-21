"""
What the audio input's capture does the same way on every platform: the
capture thread's state and stats, the input meter, waking up early when the
source in the settings changes, and finding a device by name.
"""

import math
import threading
import time
from collections.abc import Callable, Mapping
from typing import Any

import numpy as np
import numpy.typing as npt

RATE = 48000
OPEN_RETRY = 2.0  # seconds before retrying a source that failed to open
LOST_RETRY = 0.5  # ... one that was unplugged
WAIT_RETRY = 1.5  # ... a device or app that isn't there yet
CHECK_EVERY = 1.0  # seconds between checks of the default output (and the app's process)
POLL_EVERY = 10  # checks between asking for the default output anyway
GAP = 0.1  # no audio for this long: the next batch starts fresh (outputs send nothing in silence)
METER_FALL = 24.0  # dB per second the input meter falls
METER_FLOOR = -90.0
CLIP_HOLD = 2.0  # seconds the clip light stays on

# on_audio(frames, peaks, fresh)
OnAudio = Callable[[npt.NDArray[np.float32], npt.NDArray[np.float32], bool], None]


class Waiting(Exception):
    """The source can't be opened yet (its device or app isn't there)."""


class CaptureBase:
    """
    Runs the capture thread, `_loop()`, which each platform provides.
    `settings()` returns the audio input's settings (`device`, `app`,
    `app_mode`, ...). `on_audio(frames, peaks, fresh)` gets every batch of new
    audio (frames x channels float32 at RATE) with each channel's peak;
    `fresh` is True after a gap, when older samples no longer belong to
    what's playing.
    """

    def __init__(self, settings: Callable[[], Mapping[str, Any]], on_audio: OnAudio) -> None:
        self.settings = settings
        self.on_audio = on_audio
        self.running = False
        self.thread: threading.Thread | None = None
        self.state: str = 'starting'  # ok, waiting, error, starting
        self.label: str = ''  # what is being captured, for the UI
        self.kind: str = ''  # output, input, only, except
        self.message: str = ''
        self.format: tuple[int, int, bool] = (0, 0, False)  # rate, channels, event driven
        self.packets: float = 0.0  # packets per second
        self.delay: float = 0.0  # ms from capture to the engine (median)
        self.glitches: int = 0  # lost audio since the source opened
        self._meter = [(METER_FLOOR, 0.0), (METER_FLOOR, 0.0)]
        self._clip_until = 0.0

    def start(self) -> None:
        self.running = True
        self.thread = threading.Thread(target=self._loop, name='audio-input')
        self.thread.daemon = True
        self.thread.start()

    def stop(self) -> None:
        self.running = False

    def _loop(self) -> None:
        raise NotImplementedError

    # -------------------------------------------------------------- status

    def meter(self) -> tuple[list[float], bool]:
        """Peak level of the first two channels (dBFS, falling) and the clip light."""
        now = time.perf_counter()
        levels = [round(max(METER_FLOOR, db - METER_FALL * (now - t)), 1) for db, t in self._meter]
        return levels, now < self._clip_until

    def status(self) -> dict[str, Any]:
        levels, clip = self.meter()
        rate, channels, event = self.format
        return {
            'state': self.state,
            'label': self.label,
            'kind': self.kind,
            'message': self.message,
            'rate': rate,
            'channels': channels,
            'event': event,
            'packets': round(self.packets),
            'delay': round(self.delay, 1),
            'glitches': self.glitches,
            'peak': levels,
            'clip': clip,
        }

    # -------------------------------------------------------------- thread helpers

    def _wanted(self) -> tuple[Any, Any, Any]:
        audio = self.settings()
        return (audio['device'], audio['app'], audio['app_mode'])

    def _set_state(self, state: str, message: str = '') -> None:
        self.state = state
        self.message = message
        if state != 'ok':
            self.packets = 0.0

    def _sleep(self, seconds: float, wanted: tuple[Any, ...]) -> None:
        """Sleeps, but wakes up early when the source in the config changes."""
        end = time.perf_counter() + seconds
        while self.running and time.perf_counter() < end and self._wanted() == wanted:
            time.sleep(0.1)

    def _update_meter(self, peaks: npt.NDArray[np.float32], now: float) -> None:
        if not len(peaks):
            return
        if peaks.max() >= 0.999:
            self._clip_until = now + CLIP_HOLD
        pair = peaks[:2] if len(peaks) >= 2 else np.repeat(peaks[:1], 2)
        meter = list(self._meter)
        for i, peak in enumerate(pair):
            db = 20.0 * math.log10(max(float(peak), 1e-9))
            current = meter[i][0] - METER_FALL * (now - meter[i][1])
            if db >= current:
                meter[i] = (db, now)
        self._meter = meter


def find_device(found: list[dict[str, Any]], wanted: str) -> dict[str, Any] | None:
    """
    A device by name: 'in:<name>' is an input, a plain name an output (then
    an input of that name). Exact names first, then a case-insensitive part.
    """
    inputs_only = wanted.startswith('in:')
    name = wanted[3:] if inputs_only else wanted
    candidates = (
        [d for d in found if not d['output']]
        if inputs_only
        else sorted(found, key=lambda d: not d['output'])
    )
    for d in candidates:
        if d['name'] == name:
            return d
    lowered = name.lower()
    for d in candidates:
        if lowered in d['name'].lower():
            return d
    return None
