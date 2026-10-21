"""
The audio input's capture on Windows: picks the source from the settings
(an output's loopback, an input, one app, or everything except one app),
keeps it running through device switches, unplugging and app restarts, and
hands every batch of new audio on. Also keeps the input meter and capture
stats.
"""

import os
import sys
import time
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

# Capture needs Windows. Elsewhere the module still imports (tests, the
# frozen build's smoke test), lists no sources and reports an error.
if sys.platform == 'win32':
    from folo_source.inputs.audio.windows import wasapi
else:
    wasapi = None
UNSUPPORTED = 'Audio capture needs Windows'

# Never offered as apps: the audio engine itself and the studio
HIDDEN_APPS = {'audiodg.exe'}


def sources() -> dict[str, Any]:
    """
    What the UI can pick from: {'default': default output name,
    'devices': [{'name', 'output'}], 'apps': [{'exe', 'name', 'active', 'peak'}]}.
    Apps are those with audio on any output right now, playing ones first.
    """
    if wasapi is None:
        return {'default': '', 'devices': [], 'apps': []}
    wasapi.init_com()
    found, default_id = wasapi.endpoints()
    default = next((d['name'] for d in found if d['id'] == default_id), '')
    devices = [{'name': d['name'], 'output': d['output']} for d in found]
    procs = wasapi.processes()
    own = {os.getpid(), os.getppid()}
    apps = {}
    for pid, info in wasapi.sessions().items():
        if pid in own or pid not in procs:
            continue
        exe = procs[pid][0]
        if exe.lower() in HIDDEN_APPS:
            continue
        entry = apps.setdefault(
            exe.lower(),
            {'exe': exe.lower(), 'name': wasapi.app_name(pid, exe), 'active': False, 'peak': 0.0},
        )
        entry['active'] = entry['active'] or (info['active'] and info['peak'] > 0.001)
        entry['peak'] = round(max(entry['peak'], info['peak']), 3)
    ordered = sorted(apps.values(), key=lambda a: (not a['active'], a['name'].lower()))
    return {'default': default, 'devices': devices, 'apps': ordered}


class Capture(CaptureBase):
    """The capture thread on WASAPI (see CaptureBase)."""

    # -------------------------------------------------------------- sources

    def _open(self, wanted):
        """Opens the wanted source: (stream, check), check() is False once it should be reopened."""
        device, app, mode = wanted
        if app:
            procs = wasapi.processes()
            pid = wasapi.app_root(app, procs)
            name = str(wasapi.app_name(pid, procs[pid][0]) if pid else wasapi.known_app_name(app))
            only = mode != 'except'
            self.kind = 'only' if only else 'except'
            self.label = name if only else f'Everything except {name}'
            if pid is None:
                if only:
                    raise Waiting(f'Waiting for {name} to start')
                # Nothing to leave out: listen to everything until it starts
                self.label = f'Everything ({name} is not running)'
                _, default_id = wasapi.endpoints()
                stream = wasapi.open_endpoint(default_id, loopback=True, rate=RATE)
                return stream, lambda: wasapi.app_root(app) is None
            stream = wasapi.open_app(pid, include=only, rate=RATE)
            return stream, lambda: wasapi.app_root(app) == pid

        found, default_id = wasapi.endpoints()
        if device in ('', 'default'):
            match = next((d for d in found if d['id'] == default_id), None)
            if match is None:
                raise RuntimeError('No output device')
            self.kind = 'output'
            self.label = match['name']
            stream = wasapi.open_endpoint(match['id'], loopback=True, rate=RATE)
            return stream, _default_unchanged(default_id)
        match = find_device(found, device)
        if match is None:
            raise Waiting(
                '{} is not connected'.format(device[3:] if device.startswith('in:') else device)
            )
        self.kind = 'output' if match['output'] else 'input'
        self.label = match['name']
        stream = wasapi.open_endpoint(match['id'], loopback=match['output'], rate=RATE)
        return stream, lambda: True

    # -------------------------------------------------------------- thread

    def _loop(self):
        if wasapi is None:
            self._set_state('error', UNSUPPORTED)
            return
        wasapi.init_com()
        boost = wasapi.boost_thread()
        try:
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
                except wasapi.WasapiError as exc:
                    self._set_state(
                        'error', 'The device was disconnected' if exc.device_lost else str(exc)
                    )
                    self._sleep(LOST_RETRY if exc.device_lost else OPEN_RETRY, wanted)
                except Exception as exc:  # pylint: disable=broad-except
                    self._set_state('error', str(exc))
                    self._sleep(OPEN_RETRY, wanted)
                finally:
                    stream.close()
        finally:
            # Windows mustn't call the device watcher once Python shuts down
            wasapi.stop_device_changes()
            wasapi.unboost(boost)

    def _run(self, stream, wanted, check):
        self.format = (stream.rate, stream.channels, stream.event_driven)
        self.glitches = 0
        self._set_state('ok')
        first = True
        last_audio = last_check = time.perf_counter()
        count = 0
        delays = []
        while self.running and self._wanted() == wanted:
            stream.wait(max(0.02, stream.period * 2))
            packets = stream.read()
            now = time.perf_counter()
            if packets:
                frames = (
                    packets[0][0] if len(packets) == 1 else np.concatenate([p[0] for p in packets])
                )
                for _, _, flags in packets:
                    if flags & wasapi.AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY and not first:
                        self.glitches += 1
                    first = False
                delays.append((wasapi.qpc_now() - packets[-1][1]) / 1e4)
                count += len(packets)
                peaks = (
                    channel_peaks(frames)
                    if len(frames)
                    else np.zeros(frames.shape[1], dtype=np.float32)
                )
                self._update_meter(peaks, now)
                self.on_audio(frames, peaks, now - last_audio > GAP)
                last_audio = now
            if now - last_check >= CHECK_EVERY:
                self.packets = count / (now - last_check)
                self.delay = float(np.median(delays)) if delays else 0.0
                count = 0
                delays = []
                last_check = now
                if not check():
                    return


def _default_unchanged(default_id):
    """
    A check (run every CHECK_EVERY) that stays True while the default output
    is `default_id`. Asking Windows takes 2 ms, so it asks right after a
    device notification and otherwise only every POLL_EVERY checks, as a
    safety net (every third without notifications).
    """
    state = {'serial': wasapi.device_changes(), 'checks': 0}

    def check():
        state['checks'] += 1
        serial = wasapi.device_changes()
        every = 3 if serial is None else POLL_EVERY
        if serial == state['serial'] and state['checks'] % every:
            return True
        state['serial'] = serial
        return wasapi.default_output_id() == default_id

    return check
