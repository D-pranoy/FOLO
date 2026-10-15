"""
The engine's runtime (RB-03 steps 3 and 4): JSON-line logs, the stop request,
joining threads with timeouts and seedable randomness.
"""

from __future__ import annotations

import io
import json
import logging
import re
import sys
import threading
from collections.abc import Iterator
from typing import Any

import pytest

from folo_source.runtime import lifecycle, logs

TIMESTAMP = re.compile(r'^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$')


@pytest.fixture
def captured() -> Iterator[io.StringIO]:
    """logs.setup() on a buffer, with the process's logging and hooks restored after."""
    root = logging.getLogger()
    handlers, level = root.handlers[:], root.level
    hooks = sys.excepthook, threading.excepthook
    stream = io.StringIO()
    logs.setup(stream=stream)
    try:
        yield stream
    finally:
        for handler in root.handlers[:]:
            root.removeHandler(handler)
        for handler in handlers:
            root.addHandler(handler)
        root.setLevel(level)
        sys.excepthook, threading.excepthook = hooks
        logging.captureWarnings(False)


def lines(stream: io.StringIO) -> list[dict[str, Any]]:
    return [json.loads(line) for line in stream.getvalue().splitlines()]


def test_records_are_json_lines(captured: io.StringIO) -> None:
    logging.getLogger('folo_source.test').info('LEDs: %d, segments: %d', 281, 5)
    logging.getLogger('studio').warning('café — %s', 'non-ASCII')
    first, second = lines(captured)
    assert first['level'] == 'info'
    assert first['logger'] == 'folo_source.test'
    assert first['msg'] == 'LEDs: 281, segments: 5'
    assert TIMESTAMP.match(first['ts'])
    assert 'exc' not in first
    assert second['level'] == 'warning'
    assert second['msg'] == 'café — non-ASCII'
    assert captured.getvalue().isascii(), 'any console can show the lines'


def test_exceptions_carry_their_traceback(captured: io.StringIO) -> None:
    try:
        raise ValueError('bad value')
    except ValueError:
        logging.getLogger('studio').exception('could not do it')
    (entry,) = lines(captured)
    assert entry['level'] == 'error'
    assert entry['msg'] == 'could not do it'
    assert 'ValueError: bad value' in entry['exc']


def test_uncaught_exceptions_in_threads_are_logged(captured: io.StringIO) -> None:
    def fail() -> None:
        raise RuntimeError('thread failed')

    thread = threading.Thread(target=fail, name='failing')
    thread.start()
    thread.join()
    (entry,) = lines(captured)
    assert entry['level'] == 'error'
    assert entry['msg'] == 'uncaught exception in thread failing'
    assert 'RuntimeError: thread failed' in entry['exc']


def test_timestamps_are_utc_with_milliseconds() -> None:
    assert logs.timestamp(0.0) == '1970-01-01T00:00:00.000Z'
    assert logs.timestamp(1_760_000_000.9996) == '2025-10-09T08:53:20.999Z'


def test_join_waits_for_stopping_threads() -> None:
    stop = threading.Event()
    thread = threading.Thread(target=stop.wait, name='quick')
    thread.start()
    stop.set()
    assert lifecycle.join([thread, None], timeout=5.0) == []
    assert not thread.is_alive()


def test_join_gives_up_after_the_timeout(caplog: pytest.LogCaptureFixture) -> None:
    stop = threading.Event()
    stuck = threading.Thread(target=stop.wait, name='stuck', daemon=True)
    stuck.start()
    try:
        with caplog.at_level(logging.WARNING, logger=lifecycle.__name__):
            assert lifecycle.join([stuck], timeout=0.05) == ['stuck']
        assert 'still running after 0.1 s: stuck' in caplog.text
    finally:
        stop.set()
        stuck.join()


def test_join_skips_threads_never_started_and_itself() -> None:
    assert lifecycle.join([threading.Thread(), threading.current_thread()], timeout=0.0) == []


def test_a_stop_request_ends_the_wait() -> None:
    shutdown = lifecycle.Shutdown()
    assert shutdown.wait(timeout=0.05) is False
    assert not shutdown.requested
    threading.Timer(0.05, shutdown.request, args=('test',)).start()
    assert shutdown.wait(timeout=5.0) is True
    assert shutdown.requested
    assert shutdown.reason == 'test'
    shutdown.request('later')
    assert shutdown.reason == 'test', 'only the first request counts'


def test_seeded_randomness_repeats() -> None:
    first = lifecycle.random_generator(7).random(5)
    second = lifecycle.random_generator(7).random(5)
    assert first.tolist() == second.tolist()
    assert lifecycle.random_generator(8).random(5).tolist() != first.tolist()
