"""
The Linux audio capture (ADR-0031), on a fake audio server so it runs on
every platform: which device each source records, following the default
output, waiting for a device, ignoring app capture, and the listing.
test_pulse_capture.py runs it against a real PulseAudio server on Linux.
"""

from __future__ import annotations

import sys
import threading
import time
from collections.abc import Callable
from typing import Any

import numpy as np
import numpy.typing as npt
import pytest

from folo_source.inputs.audio.common import POLL_EVERY, Waiting
from folo_source.inputs.audio.linux import audio_input, pulse


class World:
    """What the fake audio server has."""

    def __init__(self) -> None:
        self.default = 'speakers'
        self.sinks: list[dict[str, Any]] = [
            {
                'name': 'speakers',
                'description': 'Speakers',
                'channels': 2,
                'monitor': 'speakers.monitor',
            },
            {'name': 'hdmi', 'description': 'HDMI Out', 'channels': 6, 'monitor': 'hdmi.monitor'},
        ]
        self.sources: list[dict[str, Any]] = [
            {'name': 'usb-mic', 'description': 'USB Mic', 'channels': 1},
        ]
        self.changes = 0
        self.up = True
        self.records: list[FakeRecord] = []


class FakeRecord:
    def __init__(self, device: str, channels: int, rate: int) -> None:
        self.device = device
        self.channels = channels
        self.rate = rate
        self.period = 0.01
        self.event_driven = True
        self.overflows = 0
        self.closed = False

    def wait(self, timeout: float) -> None:
        time.sleep(0.005)

    def read(self) -> tuple[npt.NDArray[np.float32], float]:
        return np.full((480, self.channels), 0.25, dtype=np.float32), 3.0

    def close(self) -> None:
        self.closed = True


class FakeServer:
    def __init__(self, world: World) -> None:
        if not world.up:
            raise pulse.PulseError('Connecting to the audio server failed (Connection refused)')
        self.world = world
        self.closed = False

    @property
    def changes(self) -> int:
        return self.world.changes

    @property
    def connected(self) -> bool:
        return self.world.up and not self.closed

    def server_info(self) -> dict[str, str]:
        return {'default_sink': self.world.default, 'default_source': 'usb-mic'}

    def sinks(self) -> list[dict[str, Any]]:
        return list(self.world.sinks)

    def sources(self) -> list[dict[str, Any]]:
        return list(self.world.sources)

    def subscribe(self) -> None:
        pass

    def record(self, device: str, channels: int, rate: int = 48000) -> FakeRecord:
        record = FakeRecord(device, channels, rate)
        self.world.records.append(record)
        return record

    def close(self) -> None:
        self.closed = True


@pytest.fixture
def world(monkeypatch: pytest.MonkeyPatch) -> World:
    world = World()
    monkeypatch.setattr(pulse, 'Server', lambda: FakeServer(world))
    monkeypatch.setattr(pulse, 'lib', object())  # as if libpulse loaded
    return world


def settings(device: str = 'default', app: str = '') -> dict[str, Any]:
    return {'device': device, 'app': app, 'app_mode': 'only', 'gain': 0.0, 'auto_level': False}


def capture(audio: dict[str, Any]) -> audio_input.Capture:
    return audio_input.Capture(lambda: audio, lambda frames, peaks, fresh: None)


def test_default_records_the_default_outputs_monitor(world: World) -> None:
    cap = capture(settings())
    stream, check = cap._open(('default', '', 'only'))  # pyright: ignore[reportPrivateUsage]
    assert (stream.device, stream.channels, stream.rate) == ('speakers.monitor', 2, 48000)
    assert (cap.kind, cap.label) == ('output', 'Speakers')
    assert check()
    # A server event: the default output is asked again, and it moved
    world.default = 'hdmi'
    world.changes += 1
    assert not check()


def test_a_missed_event_is_caught_by_the_safety_check(world: World) -> None:
    cap = capture(settings())
    _, check = cap._open(('default', '', 'only'))  # pyright: ignore[reportPrivateUsage]
    world.default = 'hdmi'  # without an event
    results = [check() for _ in range(POLL_EVERY)]
    assert results == [True] * (POLL_EVERY - 1) + [False]


def test_a_lost_server_reopens(world: World) -> None:
    cap = capture(settings())
    _, check = cap._open(('default', '', 'only'))  # pyright: ignore[reportPrivateUsage]
    world.up = False
    assert not check()


def test_named_outputs_and_inputs(world: World) -> None:
    cap = capture(settings())
    stream, _ = cap._open(('HDMI Out', '', 'only'))  # pyright: ignore[reportPrivateUsage]
    assert (stream.device, stream.channels, cap.kind, cap.label) == (
        'hdmi.monitor',
        6,
        'output',
        'HDMI Out',
    )
    stream, _ = cap._open(('in:usb mic', '', 'only'))  # pyright: ignore[reportPrivateUsage]
    assert (stream.device, stream.channels, cap.kind, cap.label) == (
        'usb-mic',
        1,
        'input',
        'USB Mic',
    )


def test_a_missing_device_is_waited_for(world: World) -> None:
    cap = capture(settings())
    with pytest.raises(Waiting, match='Headset is not connected'):
        cap._open(('in:Headset', '', 'only'))  # pyright: ignore[reportPrivateUsage]


def test_no_default_output_is_an_error(world: World) -> None:
    world.default = 'gone'
    cap = capture(settings())
    with pytest.raises(RuntimeError, match='No output device'):
        cap._open(('default', '', 'only'))  # pyright: ignore[reportPrivateUsage]


def test_sources_list_outputs_and_inputs(world: World) -> None:
    found = audio_input.sources()
    assert found == {
        'default': 'Speakers',
        'devices': [
            {'name': 'Speakers', 'output': True},
            {'name': 'HDMI Out', 'output': True},
            {'name': 'USB Mic', 'output': False},
        ],
        'apps': [],
    }
    world.up = False
    assert audio_input.sources() == {'default': '', 'devices': [], 'apps': []}


Batches = list[tuple[tuple[int, ...], bool]]


def _run(
    audio: dict[str, Any], until: Callable[[audio_input.Capture, Batches], bool]
) -> tuple[audio_input.Capture, Batches]:
    """Runs a capture thread until `until(cap, batches)` holds (at most 5 s)."""
    batches: Batches = []
    lock = threading.Lock()

    def on_audio(frames: npt.NDArray[np.float32], peaks: Any, fresh: bool) -> None:
        with lock:
            batches.append((frames.shape, fresh))

    cap = audio_input.Capture(lambda: audio, on_audio)
    cap.start()
    try:
        deadline = time.monotonic() + 5.0
        while not until(cap, batches):
            assert time.monotonic() < deadline, cap.status()
            time.sleep(0.02)
    finally:
        cap.stop()
        assert cap.thread is not None
        cap.thread.join(5.0)
    return cap, batches


def test_the_thread_hands_on_audio_and_ignores_apps(world: World) -> None:
    cap, batches = _run(settings(app='spotify'), lambda c, b: len(b) >= 5)
    assert all(shape == (480, 2) for shape, _ in batches)
    assert not any(fresh for _, fresh in batches)  # no gaps between them
    assert (cap.state, cap.message) == ('ok', 'App capture needs Windows')
    assert cap.format == (48000, 2, True)
    levels, clip = cap.meter()
    assert levels[0] == pytest.approx(-12.0, abs=0.5)  # 0.25 is -12 dBFS
    assert not clip
    assert world.records[-1].closed


def test_a_server_that_is_down_is_an_error(world: World) -> None:
    world.up = False
    cap, _ = _run(settings(), lambda c, b: c.state == 'error')
    assert 'Connection refused' in cap.message


def test_without_libpulse_capture_says_so(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(pulse, 'lib', None)
    cap, _ = _run(settings(), lambda c, b: c.state == 'error')
    assert cap.message == pulse.MISSING


def test_the_binding_loads_where_libpulse_is() -> None:
    # The C declarations parse everywhere; the library only loads on Linux
    assert pulse.ffi.sizeof('pa_sample_spec') == 12
    assert pulse.ffi.sizeof('pa_buffer_attr') == 20
    if sys.platform != 'linux':
        assert pulse.lib is None
        with pytest.raises(pulse.PulseError, match='libpulse'):
            pulse.Server()
