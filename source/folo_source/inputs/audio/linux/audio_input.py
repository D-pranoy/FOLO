"""
The audio input's capture on Linux (ADR-0031): picks the source from the
settings (the default output's monitor, an output's monitor or an input),
keeps it running through default output switches, unplugging and server
restarts, and hands every batch of new audio on. Also keeps the input meter
and capture stats. App capture needs Windows: here `app` is ignored.
"""

import time
from collections.abc import Callable
from typing import Any

import numpy as np

from folo_source.inputs.audio.analyzer import channel_peaks
from folo_source.inputs.audio.common import (
    CHECK_EVERY,
    GAP,
    LOST_RETRY,
    OPEN_RETRY,
    POLL_EVERY,
    RATE,
    WAIT_RETRY,
    CaptureBase,
    Waiting,
    find_device,
)
from folo_source.inputs.audio.linux import pulse

NO_APPS = 'App capture needs Windows'


def _devices(server: pulse.Server) -> list[dict[str, Any]]:
    """Every output (recorded through its monitor) and input, by description."""
    outputs = [
        {**_layout(s), 'name': s['description'], 'id': s['monitor'], 'output': True}
        for s in server.sinks()
        if s['monitor']
    ]
    inputs = [
        {**_layout(s), 'name': s['description'], 'id': s['name'], 'output': False}
        for s in server.sources()
    ]
    return outputs + inputs


def _layout(device: dict[str, Any]) -> dict[str, Any]:
    """Its channel count and layout: a stream records in the device's layout (ADR-0031)."""
    return {'channels': device['channels'], 'positions': device['positions']}


def sources() -> dict[str, Any]:
    """
    What the UI can pick from: {'default': default output name,
    'devices': [{'name', 'output'}], 'apps': []}. Without an audio server
    there's nothing to pick.
    """
    try:
        server = pulse.Server()
    except pulse.PulseError:
        return {'default': '', 'devices': [], 'apps': []}
    try:
        default = server.server_info().get('default_sink', '')
        sinks = server.sinks()
        devices = _devices(server)
    except pulse.PulseError:
        return {'default': '', 'devices': [], 'apps': []}
    finally:
        server.close()
    name = next((s['description'] for s in sinks if s['name'] == default), '')
    return {
        'default': name,
        'devices': [{'name': d['name'], 'output': d['output']} for d in devices],
        'apps': [],
    }


class Capture(CaptureBase):
    """The capture thread on the PulseAudio client library (see CaptureBase)."""

    # -------------------------------------------------------------- sources

    def _open(self, wanted: tuple[Any, ...]) -> tuple[Any, Callable[[], bool]]:
        """Opens the wanted source: (stream, check), check() is False once it should be reopened."""
        device, _app, _mode = wanted
        server = pulse.Server()
        try:
            server.subscribe()
            if device in ('', 'default'):
                default = server.server_info().get('default_sink', '')
                sink = next((s for s in server.sinks() if s['name'] == default), None)
                if sink is None or not sink['monitor']:
                    raise Waiting('No output device')
                self.kind = 'output'
                self.label = sink['description']
                stream = server.record(sink['monitor'], sink['channels'], RATE, sink['positions'])
                return stream, _default_unchanged(server, default)
            match = find_device(_devices(server), device)
            if match is None:
                raise Waiting(
                    '{} is not connected'.format(device[3:] if device.startswith('in:') else device)
                )
            self.kind = 'output' if match['output'] else 'input'
            self.label = match['name']
            stream = server.record(match['id'], match['channels'], RATE, match['positions'])
            return stream, lambda: server.connected
        except BaseException:
            server.close()
            raise

    # -------------------------------------------------------------- thread

    def _loop(self) -> None:
        if pulse.lib is None:
            self._set_state('error', pulse.MISSING)
            return
        while self.running:
            wanted = self._wanted()
            try:
                stream, check = self._open(wanted)
            except Waiting as exc:
                self._set_state('waiting', str(exc))
                self._sleep(WAIT_RETRY, wanted)
                continue
            except Exception as exc:  # pylint: disable=broad-except
                self._set_state('error', str(exc))
                self._sleep(OPEN_RETRY, wanted)
                continue
            try:
                self._run(stream, wanted, check)
            except pulse.PulseError as exc:
                self._set_state('error', str(exc))
                self._sleep(LOST_RETRY if exc.lost else OPEN_RETRY, wanted)
            except Exception as exc:  # pylint: disable=broad-except
                self._set_state('error', str(exc))
                self._sleep(OPEN_RETRY, wanted)
            finally:
                stream.close()

    def _run(self, stream: Any, wanted: tuple[Any, ...], check: Callable[[], bool]) -> None:
        self.format = (stream.rate, stream.channels, stream.event_driven)
        self.glitches = 0
        self._set_state('ok', NO_APPS if wanted[1] else '')
        last_audio = last_check = time.perf_counter()
        count = 0
        delays: list[float] = []
        while self.running and self._wanted() == wanted:
            stream.wait(max(0.02, stream.period * 2))
            frames, latency = stream.read()
            now = time.perf_counter()
            if frames is not None and len(frames):
                self.glitches = stream.overflows
                delays.append(latency)
                count += max(1, round(len(frames) / (stream.rate * stream.period)))
                peaks = channel_peaks(frames)
                self._update_meter(peaks, now)
                self.on_audio(frames, peaks, now - last_audio > GAP)
                last_audio = now
            if now - last_check >= CHECK_EVERY:
                self.packets = count / (now - last_check)
                self.delay = float(np.median(delays)) if delays else 0.0
                count = 0
                delays: list[float] = []
                last_check = now
                if not check():
                    return


def _default_unchanged(server: Any, default: str) -> Callable[[], bool]:
    """
    A check (run every CHECK_EVERY) that stays True while the server's
    default output is `default`. It asks the server after any server, sink
    or source event, and every POLL_EVERY checks in case one was missed.
    """
    state = {'changes': server.changes, 'checks': 0}

    def check() -> bool:
        if not server.connected:
            return False
        state['checks'] += 1
        changes = server.changes
        if changes == state['changes'] and state['checks'] % POLL_EVERY:
            return True
        state['changes'] = changes
        return server.server_info().get('default_sink', '') == default

    return check
