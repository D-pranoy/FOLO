"""
The studio stops cleanly (RB-03 steps 3 and 4): on SIGTERM, and on Windows on
Ctrl+C, Ctrl+Break and when its console is closed, it stops its threads,
saves the config and logs "stopped", and everything it writes is JSON lines.

On Windows the studio gets a hidden console of its own, and a helper process
attaches to it to send the event, so the test needs no console itself.
"""

from __future__ import annotations

import json
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

from tests.test_studio_starts import ROOT, START_TIMEOUT, free_port, get

STOP_TIMEOUT = 15.0

# Sends a console event to the console of the process given: 'ctrl-c' or
# 'break' (to everything attached; the helper ignores Ctrl+C itself) or
# 'close' (closes the console window)
HELPER = r"""
import ctypes, sys
kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
user32 = ctypes.WinDLL('user32', use_last_error=True)
pid, event = int(sys.argv[1]), sys.argv[2]
kernel32.FreeConsole()
if not kernel32.AttachConsole(pid):
    sys.exit(f'AttachConsole failed: {ctypes.get_last_error()}')
if event in ('ctrl-c', 'break'):
    kernel32.SetConsoleCtrlHandler(None, True)
    if not kernel32.GenerateConsoleCtrlEvent(0 if event == 'ctrl-c' else 1, 0):
        sys.exit(f'GenerateConsoleCtrlEvent failed: {ctypes.get_last_error()}')
else:
    hwnd = kernel32.GetConsoleWindow()
    kernel32.FreeConsole()
    if not hwnd or not user32.PostMessageW(hwnd, 0x0010, 0, 0):
        sys.exit(f'closing the console failed: {ctypes.get_last_error()}')
"""

STOPS = ['ctrl-c', 'break', 'close'] if sys.platform == 'win32' else ['sigterm']


def _start(tmp_path: Path, log: Path) -> tuple[subprocess.Popen[bytes], str]:
    config = tmp_path / 'studio.json'
    config.write_text(
        json.dumps({'hyperion': {'enabled': False, 'host': '127.0.0.1', 'port': 9}}),
        encoding='utf-8',
    )
    port = free_port()
    command = [sys.executable, 'studio.py', '--port', str(port), '--no-browser']
    command += ['--config', str(config)]
    creationflags, startup = 0, None
    if sys.platform == 'win32':
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0  # SW_HIDE
        creationflags = subprocess.CREATE_NEW_CONSOLE
    with log.open('wb') as out:
        proc = subprocess.Popen(
            command,
            cwd=ROOT,
            stdout=out,
            stderr=subprocess.STDOUT,
            creationflags=creationflags,
            startupinfo=startup,
        )
    return proc, f'http://127.0.0.1:{port}'


def _wait_until_serving(proc: subprocess.Popen[bytes], base: str, log: Path) -> None:
    deadline = time.monotonic() + START_TIMEOUT
    while True:
        assert proc.poll() is None, log.read_text(encoding='utf-8', errors='replace')
        try:
            get(f'{base}/api/state')
            return
        except OSError:
            assert time.monotonic() < deadline, 'the studio did not start in time'
            time.sleep(0.25)


def _log_entries(log: Path) -> list[dict[str, Any]]:
    text = log.read_text(encoding='utf-8', errors='replace')
    entries: list[dict[str, Any]] = []
    for line in text.splitlines():
        try:
            entries.append(json.loads(line))
        except ValueError:
            pytest.fail(f'not a JSON line: {line!r}\n{text}')
    return entries


def _stopped(log: Path) -> bool:
    text = log.read_text(encoding='utf-8', errors='replace')
    return any(json.loads(line).get('msg') == 'stopped' for line in text.splitlines() if line)


@pytest.mark.parametrize('stop', STOPS)
def test_studio_stops_cleanly(tmp_path: Path, stop: str) -> None:
    log = tmp_path / 'studio.log'
    proc, base = _start(tmp_path, log)
    try:
        _wait_until_serving(proc, base, log)
        if stop == 'sigterm':
            proc.send_signal(signal.SIGTERM)
        else:
            helper = subprocess.run(
                [sys.executable, '-c', HELPER, str(proc.pid), stop],
                capture_output=True,
                timeout=30,
                check=False,
            )
            # Ctrl+Break reaches the helper too, so only a failure it reports counts
            assert b'failed' not in helper.stderr, helper.stderr
        deadline = time.monotonic() + STOP_TIMEOUT
        while not _stopped(log):
            assert time.monotonic() < deadline, log.read_text(encoding='utf-8', errors='replace')
            time.sleep(0.1)
        code = proc.wait(timeout=STOP_TIMEOUT)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=30)

    entries = _log_entries(log)
    messages = [e['msg'] for e in entries]
    assert any(m.startswith('stopping: ') for m in messages), messages
    assert messages[-1] == 'stopped', messages
    assert not [e for e in entries if e['level'] in ('error', 'critical')], entries
    assert not [m for m in messages if m.startswith('still running')], messages
    if stop != 'close':
        # Closing the console ends the launcher at once; its code means nothing then
        assert code == 0, (code, messages)
    saved = json.loads((tmp_path / 'studio.json').read_text(encoding='utf-8'))
    assert saved['hyperion']['enabled'] is False
