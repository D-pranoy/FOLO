"""
The conformance harness (scripts/conformance.py, RB-02 step 5): the example
messages in specs/fixtures/ conform, and the harness notices messages,
frames and API responses that don't.
"""

from __future__ import annotations

import json
import os
import shutil
import struct
import subprocess
import sys
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from scripts import conformance

ROOT = Path(__file__).resolve().parent.parent


def test_fixtures_cover_every_message() -> None:
    covered = {conformance.message_of(p) for p in conformance.message_files(conformance.FIXTURES)}
    assert set(conformance.message_names()) <= covered


def test_fixtures_conform() -> None:
    assert conformance.check_messages(conformance.FIXTURES) == []


def write(folder: Path, name: str, payload: object) -> Path:
    path = folder / name
    path.write_text(json.dumps(payload), encoding='utf-8')
    return path


def test_bad_messages_are_caught(tmp_path: Path) -> None:
    fixture = json.loads((conformance.FIXTURES / 'config.apply.json').read_text(encoding='utf-8'))
    del fixture['params']['publish']
    write(tmp_path, 'config.apply--no-publish.json', fixture)
    write(tmp_path, 'subscribe--bad-topic.json', {'subscribe': ['nonsense topic']})
    write(tmp_path, 'nothing.json', {})
    problems = conformance.check_messages(tmp_path)
    assert any('config.apply--no-publish' in p and 'publish' in p for p in problems)
    assert any('subscribe--bad-topic' in p for p in problems)
    assert any('nothing.json: no AsyncAPI message' in p for p in problems)


def test_bad_frames_are_caught() -> None:
    good = (conformance.FIXTURES / 'surfaceFrame--rgb8.fols').read_bytes()
    assert conformance.check_frame(good, 'surface') == []
    assert any('magic' in p for p in conformance.check_frame(b'XXXX' + good[4:], 'surface'))
    assert any('payload' in p for p in conformance.check_frame(good[:-3], 'surface'))
    assert any(
        'reserved' in p for p in conformance.check_frame(good[:7] + b'\x01' + good[8:], 'surface')
    )
    signal = struct.pack('<4sBBHHHIQ', b'FOLG', 1, 0, 0, 2, 0, 1, 1) + struct.pack('<f', 1.0)
    assert any('payload' in p for p in conformance.check_frame(signal, 'signal'))


class Stub(BaseHTTPRequestHandler):
    """Just /api/v1/health; `health` is what it answers."""

    health: object = {'status': 'ok'}

    def log_message(self, format: str, *args: object) -> None:
        pass

    def reply(self, status: int, body: object) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        if status == 405:
            self.send_header('Allow', 'GET')
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        if self.path.split('?')[0] == '/api/v1/health':
            self.reply(200, self.health)
        else:
            self.reply(404, {'error': {'code': 'not_found', 'message': 'Not found'}})

    def other(self) -> None:
        self.reply(405, {'error': {'code': 'method_not_allowed', 'message': 'Not allowed'}})

    def __getattr__(self, name: str) -> object:
        # Every other method, including ones http.server doesn't know (it would answer 501)
        if name.startswith('do_'):
            return self.other
        raise AttributeError(name)


def serve(health: object) -> Iterator[str]:
    handler = type('Handler', (Stub,), {'health': health})
    server = ThreadingHTTPServer(('127.0.0.1', 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_address[1]}/api/v1'
    finally:
        server.shutdown()
        server.server_close()


def run_api(url: str) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, 'PYTHONUTF8': '1', 'PYTHONIOENCODING': 'utf-8'}
    return subprocess.run(
        [
            sys.executable,
            str(ROOT / 'scripts' / 'conformance.py'),
            'api',
            '--url',
            url,
            '--include-path',
            '/health',
            '--max-examples',
            '5',
            '--seed',
            '1',
        ],
        capture_output=True,
        text=True,
        encoding='utf-8',
        check=False,
        env=env,
    )


@pytest.mark.skipif(shutil.which('st') is None, reason='needs schemathesis on PATH')
def test_api_harness_passes_a_conforming_server() -> None:
    for url in serve({'status': 'ok'}):
        result = run_api(url)
        assert result.returncode == 0, result.stdout[-3000:]


@pytest.mark.skipif(shutil.which('st') is None, reason='needs schemathesis on PATH')
def test_api_harness_fails_a_wrong_response() -> None:
    for url in serve({'status': 'broken'}):
        result = run_api(url)
        # 1 is "the API failed a check"; anything else would be the tool failing
        assert result.returncode == 1, result.stdout[-3000:]
        assert 'schema' in result.stdout.lower()
