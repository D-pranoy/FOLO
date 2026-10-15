"""
The engine's logs: stdlib logging, written to stderr as JSON lines for the
node to collect (docs/architecture/control-plane.md#supervision). Each line
is one object: `ts` (UTC, RFC 3339 with milliseconds), `level`, `logger`,
`msg`, and `exc` with the traceback when there is one.
"""

from __future__ import annotations

import json
import logging
import sys
import threading
import time
from types import TracebackType
from typing import TextIO

log = logging.getLogger(__name__)


def timestamp(created: float) -> str:
    """A record's time as RFC 3339 in UTC, with milliseconds."""
    seconds = int(created)
    millis = min(999, int((created - seconds) * 1000))
    return time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(seconds)) + f'.{millis:03d}Z'


class JsonLines(logging.Formatter):
    """One JSON object per record, ASCII only, so any console can show it."""

    def format(self, record: logging.LogRecord) -> str:
        entry = {
            'ts': timestamp(record.created),
            'level': record.levelname.lower(),
            'logger': record.name,
            'msg': record.getMessage(),
        }
        if record.exc_info:
            entry['exc'] = self.formatException(record.exc_info)
        if record.stack_info:
            entry['stack'] = self.formatStack(record.stack_info)
        return json.dumps(entry)


def setup(level: int | str = logging.INFO, stream: TextIO | None = None) -> logging.Handler:
    """
    Sends the process's log records to `stream` (stderr) as JSON lines,
    replacing any handlers set up before. Uncaught exceptions, in the main
    thread or any other, are logged too instead of printed.
    """
    handler = logging.StreamHandler(sys.stderr if stream is None else stream)
    handler.setFormatter(JsonLines())
    root = logging.getLogger()
    for old in root.handlers[:]:
        root.removeHandler(old)
    root.addHandler(handler)
    root.setLevel(level)
    logging.captureWarnings(True)
    sys.excepthook = _log_uncaught
    threading.excepthook = _log_uncaught_in_thread
    return handler


def _log_uncaught(
    kind: type[BaseException], value: BaseException, traceback: TracebackType | None
) -> None:
    log.critical('uncaught exception', exc_info=(kind, value, traceback))


def _log_uncaught_in_thread(args: threading.ExceptHookArgs) -> None:
    if args.exc_type is SystemExit:
        return
    name = args.thread.name if args.thread is not None else 'unknown'
    exc_value = args.exc_value if args.exc_value is not None else args.exc_type()
    log.error(
        'uncaught exception in thread %s',
        name,
        exc_info=(args.exc_type, exc_value, args.exc_traceback),
    )
