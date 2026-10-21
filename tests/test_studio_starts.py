"""
The studio starts on every platform (RB-01 step 5): `studio.py` serves the
page, the state and the stream with Hyperion output off. On Linux it captures
through libpulse where an audio server answers (ADR-0031); without one, and
outside Windows and Linux, capture says why instead of failing.

The test config points Hyperion at a closed local port, so the studio never
talks to a real Hyperion.
"""

from __future__ import annotations

import json
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
START_TIMEOUT = 60.0


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return int(sock.getsockname()[1])


def get(url: str, timeout: float = 5.0) -> bytes:
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return response.read()


def _stream_messages(url: str, count: int) -> list[dict[str, Any]]:
    """The first `count` messages of the stream (30 a second)."""
    messages: list[dict[str, Any]] = []
    with urllib.request.urlopen(url, timeout=10) as response:
        for raw in response:
            line = raw.decode('utf-8').strip()
            if line.startswith('data:'):
                messages.append(json.loads(line[5:]))
                if len(messages) == count:
                    break
    assert messages, 'the stream sent no message'
    return messages


def test_studio_starts(tmp_path: Path) -> None:
    config = tmp_path / 'studio.json'
    config.write_text(
        json.dumps({'hyperion': {'enabled': False, 'host': '127.0.0.1', 'port': 9}}),
        encoding='utf-8',
    )
    port = free_port()
    base = f'http://127.0.0.1:{port}'
    log = tmp_path / 'studio.log'
    with log.open('wb') as out:
        proc = subprocess.Popen(
            [
                sys.executable,
                'studio.py',
                '--port',
                str(port),
                '--no-browser',
                '--config',
                str(config),
            ],
            cwd=ROOT,
            stdout=out,
            stderr=subprocess.STDOUT,
        )
        try:
            deadline = time.monotonic() + START_TIMEOUT
            state: dict[str, Any] | None = None
            while state is None:
                assert proc.poll() is None, log.read_text(encoding='utf-8', errors='replace')
                try:
                    state = json.loads(get(f'{base}/api/state'))
                except OSError:
                    assert time.monotonic() < deadline, 'the studio did not start in time'
                    time.sleep(0.25)
            assert state['config']['hyperion']['enabled'] is False
            assert b'<html' in get(f'{base}/').lower()
            # About three seconds of the stream: the input thread sets its state meanwhile
            messages = _stream_messages(f'{base}/api/stream', 90)
            assert all(len(m['leds']) == 6 * m['led_count'] for m in messages)
            inputs = {
                (m['status']['input']['state'], m['status']['input']['message']) for m in messages
            }
            if sys.platform == 'linux':
                assert {state for state, _ in inputs} & {'ok', 'waiting', 'error'}, inputs
            elif sys.platform != 'win32':
                assert state['devices'] == []
                assert ('error', 'Audio capture needs Windows') in inputs, inputs
        finally:
            proc.terminate()
            proc.wait(timeout=30)
