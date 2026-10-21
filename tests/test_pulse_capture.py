"""
The Linux audio capture against a real PulseAudio server (ADR-0031): a tone
played into a null sink must arrive through its monitor, and a default sink
change must reopen the stream. No audio device is needed.

Runs on Linux with a server reachable and `pactl` and `pacat` installed. CI
sets FOLO_PULSE_TESTS=1, which turns a missing server into a failure instead
of a skip.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator
from typing import Any

import numpy as np
import numpy.typing as npt
import pytest

pytestmark = pytest.mark.skipif(sys.platform != 'linux', reason='PulseAudio runs on Linux')

# Descriptions without spaces: module arguments split on them
SINKS = {'folo_test_a': 'FOLO-test-A', 'folo_test_b': 'FOLO-test-B'}
AMPLITUDE = 0.5
RATE = 48000


def _pactl(*args: str) -> str:
    return subprocess.run(
        ['pactl', *args], check=True, capture_output=True, text=True, timeout=10
    ).stdout.strip()


@pytest.fixture
def null_sinks() -> Iterator[None]:
    """Two null sinks, the first one the default; removed afterwards."""
    required = os.environ.get('FOLO_PULSE_TESTS') == '1'
    try:
        if shutil.which('pactl') is None or shutil.which('pacat') is None:
            raise OSError('pactl and pacat are not installed')
        _pactl('info')
    except (OSError, subprocess.SubprocessError) as exc:
        if required:
            pytest.fail(f'no PulseAudio server: {exc}')
        pytest.skip(f'no PulseAudio server: {exc}')
    previous = _pactl('get-default-sink')
    modules = [
        _pactl(
            'load-module',
            'module-null-sink',
            f'sink_name={name}',
            f'sink_properties=device.description={description}',
        )
        for name, description in SINKS.items()
    ]
    _pactl('set-default-sink', 'folo_test_a')
    try:
        yield
    finally:
        for module in modules:
            _pactl('unload-module', module)
        if previous and previous not in SINKS:
            subprocess.run(['pactl', 'set-default-sink', previous], check=False, timeout=10)


def _play(sink: str, seconds: float) -> subprocess.Popen[bytes]:
    """Plays a 1 kHz stereo tone at AMPLITUDE into `sink`."""
    t = np.arange(round(RATE * seconds)) / RATE
    mono = (AMPLITUDE * np.sin(2 * np.pi * 1000.0 * t)).astype(np.float32)
    data = np.column_stack((mono, mono)).astype('<f4').tobytes()
    player = subprocess.Popen(
        [
            'pacat',
            '--playback',
            f'--device={sink}',
            '--format=float32le',
            f'--rate={RATE}',
            '--channels=2',
        ],
        stdin=subprocess.PIPE,
    )
    assert player.stdin is not None
    player.stdin.write(data)
    player.stdin.close()
    return player


class Listener:
    """A capture thread that keeps each batch's loudest peak."""

    def __init__(self, device: str) -> None:
        from folo_source.inputs.audio.linux import audio_input

        self.peaks: list[float] = []
        self.lock = threading.Lock()
        self.audio = {'device': device, 'app': '', 'app_mode': 'only'}
        self.capture = audio_input.Capture(lambda: self.audio, self.on_audio)

    def on_audio(self, frames: npt.NDArray[np.float32], peaks: Any, fresh: bool) -> None:
        with self.lock:
            self.peaks.append(float(np.max(peaks)))

    def loudest(self) -> float:
        with self.lock:
            return max(self.peaks, default=0.0)

    def clear(self) -> None:
        with self.lock:
            self.peaks.clear()

    def wait(self, condition: Callable[[], bool], seconds: float, what: str) -> None:
        deadline = time.monotonic() + seconds
        while not condition():
            assert time.monotonic() < deadline, f'{what}: {self.capture.status()}'
            time.sleep(0.05)

    def __enter__(self) -> Listener:
        self.capture.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self.capture.stop()
        assert self.capture.thread is not None
        self.capture.thread.join(5.0)


@pytest.mark.usefixtures('null_sinks')
def test_sources_list_the_null_sinks() -> None:
    from folo_source.inputs.audio.linux import audio_input

    found = audio_input.sources()
    assert found['default'] == 'FOLO-test-A'
    outputs = [d['name'] for d in found['devices'] if d['output']]
    assert set(SINKS.values()) <= set(outputs)
    # The monitors are how outputs are recorded, never listed as inputs
    assert not any('Monitor' in d['name'] for d in found['devices'] if not d['output'])


@pytest.mark.usefixtures('null_sinks')
def test_a_tone_arrives_through_the_monitor() -> None:
    with Listener('FOLO-test-A') as listener:
        listener.wait(lambda: listener.capture.state == 'ok', 10.0, 'the stream did not open')
        assert listener.capture.label == 'FOLO-test-A'
        assert listener.capture.format == (RATE, 2, True)
        listener.wait(lambda: len(listener.peaks) >= 10, 5.0, 'no audio from the monitor')
        assert listener.loudest() < 0.01  # silence before the tone
        player = _play('folo_test_a', 3.0)
        try:
            listener.wait(
                lambda: listener.loudest() > AMPLITUDE * 0.9, 10.0, 'the tone did not arrive'
            )
        finally:
            player.wait(10)
        assert listener.loudest() < AMPLITUDE * 1.1
        assert listener.capture.packets > 50  # about 100 a second
        assert listener.capture.delay < 200.0


@pytest.mark.usefixtures('null_sinks')
def test_a_default_sink_change_reopens_the_stream() -> None:
    with Listener('default') as listener:
        listener.wait(
            lambda: listener.capture.state == 'ok' and listener.capture.label == 'FOLO-test-A',
            10.0,
            'the default output did not open',
        )
        _pactl('set-default-sink', 'folo_test_b')
        listener.wait(
            lambda: listener.capture.label == 'FOLO-test-B', 15.0, 'the stream did not follow'
        )
        listener.wait(lambda: listener.capture.state == 'ok', 10.0, 'the new stream did not open')
        listener.clear()
        player = _play('folo_test_b', 3.0)
        try:
            listener.wait(
                lambda: listener.loudest() > AMPLITUDE * 0.9,
                10.0,
                'the tone on the new default did not arrive',
            )
        finally:
            player.wait(10)
