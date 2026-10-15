"""
The engine's process lifecycle: SIGINT, SIGTERM and Windows' console control
events (Ctrl+C, Ctrl+Break, closing the console, signing out, shutting down)
all become one request to stop. The main thread waits for it, stops the
engine, joins its threads with timeouts and says when it's done.
"""

from __future__ import annotations

import logging
import signal
import sys
import threading
import time
from collections.abc import Iterable
from types import FrameType
from typing import Any

import numpy as np

log = logging.getLogger(__name__)

JOIN_TIMEOUT = 3.0  # seconds a stopping thread gets to finish
# Windows ends a console process this long after closing its console (5 s),
# so the handler gives the main thread a little less to clean up
CONSOLE_GRACE = 4.5
# The console events handled here (Ctrl+C and Ctrl+Break arrive as SIGINT and
# SIGBREAK through Python's own handler)
CONSOLE_EVENTS = {2: 'the console was closed', 5: 'signing out', 6: 'shutting down'}
WAIT_STEP = 0.2  # the main thread wakes up this often to run signal handlers


class Shutdown:
    """
    One request to stop for the whole process. `install()` hooks the signals
    and console events, the main thread blocks in `wait()`, and calls
    `done()` once it has cleaned up, which lets a console event handler
    return (Windows ends the process when it does).
    """

    def __init__(self) -> None:
        self._requested = threading.Event()
        self._done = threading.Event()
        self.reason = ''
        self._console_handler: Any = None  # kept alive while Windows may call it

    @property
    def requested(self) -> bool:
        return self._requested.is_set()

    def request(self, reason: str) -> None:
        """Asks the process to stop. Only the first request counts."""
        if not self._requested.is_set():
            self.reason = reason
            log.info('stopping: %s', reason)
            self._requested.set()

    def install(self) -> None:
        """Hooks SIGINT, SIGTERM, SIGBREAK and the console events. Main thread only."""
        for name in ('SIGINT', 'SIGTERM', 'SIGBREAK'):
            signum = getattr(signal, name, None)
            if signum is not None:
                signal.signal(signum, self._on_signal)
        if sys.platform == 'win32':
            self._install_console_handler()

    def wait(self, timeout: float | None = None) -> bool:
        """
        Blocks until a stop is requested (True) or `timeout` seconds pass
        (False). Wakes up regularly, so the main thread runs signal handlers.
        """
        deadline = None if timeout is None else time.monotonic() + timeout
        while not self._requested.wait(WAIT_STEP):
            if deadline is not None and time.monotonic() >= deadline:
                return False
        return True

    def done(self) -> None:
        """The process has cleaned up and may end."""
        self._done.set()

    def _on_signal(self, signum: int, frame: FrameType | None) -> None:
        self.request(signal.Signals(signum).name)

    def _install_console_handler(self) -> None:
        import ctypes
        from ctypes import wintypes

        def handler(event: int) -> bool:
            if event not in CONSOLE_EVENTS:
                return False  # the next handler: Python's, for Ctrl+C and Ctrl+Break
            self.request(CONSOLE_EVENTS[event])
            self._done.wait(CONSOLE_GRACE)
            return True

        kernel32 = ctypes.windll.kernel32
        # A process inherits "ignore Ctrl+C" from its parent (one started in a
        # new process group, for example): take Ctrl+C again
        kernel32.SetConsoleCtrlHandler(None, False)
        self._console_handler = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.DWORD)(handler)
        if not kernel32.SetConsoleCtrlHandler(self._console_handler, True):
            log.warning('could not watch for console events')


def join(threads: Iterable[threading.Thread | None], timeout: float = JOIN_TIMEOUT) -> list[str]:
    """
    Joins threads that have been told to stop, all within one `timeout`.
    Returns the names of those still running, and logs them.
    """
    deadline = time.monotonic() + timeout
    late: list[str] = []
    for thread in threads:
        if thread is None or thread is threading.current_thread() or not thread.ident:
            continue
        thread.join(max(0.0, deadline - time.monotonic()))
        if thread.is_alive():
            late.append(thread.name)
    if late:
        log.warning('still running after %.1f s: %s', timeout, ', '.join(late))
    return late


def random_generator(seed: int | None = None) -> np.random.Generator:
    """The engine's random numbers (drops, fire): seeded for repeatable runs, else fresh."""
    return np.random.default_rng(seed)
