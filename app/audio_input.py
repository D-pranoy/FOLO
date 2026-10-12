"""
The studio's audio input: picks the source from the config (an output's
loopback, an input, one app, or everything except one app), keeps it running
through device switches, unplugging and app restarts, and hands every batch
of new audio to the engine. Also keeps the input meter and capture stats.
"""

import math
import os
import threading
import time

import numpy as np

from app import wasapi

RATE = 48000
OPEN_RETRY = 2.0            # seconds before retrying a source that failed to open
LOST_RETRY = 0.5            # ... one that was unplugged
WAIT_RETRY = 1.5            # ... an app that isn't running yet
CHECK_EVERY = 1.0           # seconds between checks of the default output and the app's process
POLL_EVERY = 10             # checks between asking Windows for the default output anyway
GAP = 0.1                   # no audio for this long: the next batch starts fresh (outputs send nothing in silence)
METER_FALL = 24.0           # dB per second the input meter falls
METER_FLOOR = -90.0
CLIP_HOLD = 2.0             # seconds the clip light stays on
# Never offered as apps: the audio engine itself and the studio
HIDDEN_APPS = {'audiodg.exe'}


def sources():
    """
    What the UI can pick from: {'default': default output name,
    'devices': [{'name', 'output'}], 'apps': [{'exe', 'name', 'active', 'peak'}]}.
    Apps are those with audio on any output right now, playing ones first.
    """
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
        entry = apps.setdefault(exe.lower(), {'exe': exe.lower(), 'name': wasapi.app_name(pid, exe),
                                              'active': False, 'peak': 0.0})
        entry['active'] = entry['active'] or (info['active'] and info['peak'] > 0.001)
        entry['peak'] = round(max(entry['peak'], info['peak']), 3)
    ordered = sorted(apps.values(), key=lambda a: (not a['active'], a['name'].lower()))
    return {'default': default, 'devices': devices, 'apps': ordered}


class Waiting(Exception):
    """The source can't be opened yet (its app isn't running)."""


class AudioInput(object):
    """
    Runs the capture thread. `on_audio(frames, peaks, fresh)` gets every
    batch of new audio (frames x channels float32 at RATE) with each
    channel's peak; `fresh` is True after a gap, when older samples no longer
    belong to what's playing.
    """
    def __init__(self, store, on_audio):
        self.store = store
        self.on_audio = on_audio
        self.running = False
        self.thread = None
        self.state = 'starting'         # ok, waiting, error, starting
        self.label = ''                 # what is being captured, for the UI
        self.kind = ''                  # output, input, only, except
        self.message = ''
        self.format = (0, 0, False)     # rate, channels, event driven
        self.packets = 0.0              # packets per second
        self.delay = 0.0                # ms from WASAPI's capture time to the engine (median)
        self.glitches = 0               # lost audio (data discontinuities) since the source opened
        self._meter = [(METER_FLOOR, 0.0), (METER_FLOOR, 0.0)]
        self._clip_until = 0.0

    def start(self):
        self.running = True
        self.thread = threading.Thread(target=self._loop, name='audio-input')
        self.thread.daemon = True
        self.thread.start()

    def stop(self):
        self.running = False

    # -------------------------------------------------------------- status

    def meter(self):
        """Peak level of the first two channels (dBFS, falling) and the clip light."""
        now = time.perf_counter()
        levels = [round(max(METER_FLOOR, db - METER_FALL * (now - t)), 1) for db, t in self._meter]
        return levels, now < self._clip_until

    def status(self):
        levels, clip = self.meter()
        rate, channels, event = self.format
        return {
            'state': self.state,
            'label': self.label,
            'kind': self.kind,
            'message': self.message,
            'rate': rate,
            'channels': channels,
            'event': event,
            'packets': round(self.packets),
            'delay': round(self.delay, 1),
            'glitches': self.glitches,
            'peak': levels,
            'clip': clip,
        }

    # -------------------------------------------------------------- sources

    def _wanted(self):
        audio = self.store.config['audio']
        return (audio['device'], audio['app'], audio['app_mode'])

    def _open(self, wanted):
        """Opens the wanted source: (stream, check), check() is False once it should be reopened."""
        device, app, mode = wanted
        if app:
            procs = wasapi.processes()
            pid = wasapi.app_root(app, procs)
            name = wasapi.app_name(pid, procs[pid][0]) if pid else wasapi.known_app_name(app)
            only = mode != 'except'
            self.kind = 'only' if only else 'except'
            self.label = name if only else 'Everything except {}'.format(name)
            if pid is None:
                if only:
                    raise Waiting('Waiting for {} to start'.format(name))
                # Nothing to leave out: listen to everything until it starts
                self.label = 'Everything ({} is not running)'.format(name)
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
        match = _find_device(found, device)
        if match is None:
            raise Waiting('{} is not connected'.format(device[3:] if device.startswith('in:') else device))
        self.kind = 'output' if match['output'] else 'input'
        self.label = match['name']
        stream = wasapi.open_endpoint(match['id'], loopback=match['output'], rate=RATE)
        return stream, lambda: True

    # -------------------------------------------------------------- thread

    def _loop(self):
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
                except Exception as exc: # pylint: disable=broad-except
                    self._set_state('error', str(exc))
                    self._sleep(OPEN_RETRY, wanted)
                    continue
                try:
                    self._run(stream, wanted, check)
                except wasapi.WasapiError as exc:
                    self._set_state('error', 'The device was disconnected' if exc.device_lost else str(exc))
                    self._sleep(LOST_RETRY if exc.device_lost else OPEN_RETRY, wanted)
                except Exception as exc: # pylint: disable=broad-except
                    self._set_state('error', str(exc))
                    self._sleep(OPEN_RETRY, wanted)
                finally:
                    stream.close()
        finally:
            wasapi.unboost(boost)

    def _set_state(self, state, message=''):
        self.state = state
        self.message = message
        if state != 'ok':
            self.packets = 0.0

    def _sleep(self, seconds, wanted):
        """Sleeps, but wakes up early when the source in the config changes."""
        end = time.perf_counter() + seconds
        while self.running and time.perf_counter() < end and self._wanted() == wanted:
            time.sleep(0.1)

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
                frames = packets[0][0] if len(packets) == 1 else np.concatenate([p[0] for p in packets])
                for _, _, flags in packets:
                    if flags & wasapi.AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY and not first:
                        self.glitches += 1
                    first = False
                delays.append((wasapi.qpc_now() - packets[-1][1]) / 1e4)
                count += len(packets)
                peaks = np.abs(frames).max(axis=0) if len(frames) else np.zeros(frames.shape[1], dtype=np.float32)
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

    def _update_meter(self, peaks, now):
        if not len(peaks):
            return
        if peaks.max() >= 0.999:
            self._clip_until = now + CLIP_HOLD
        pair = peaks[:2] if len(peaks) >= 2 else np.repeat(peaks[:1], 2)
        meter = list(self._meter)
        for i, peak in enumerate(pair):
            db = 20.0 * math.log10(max(float(peak), 1e-9))
            current = meter[i][0] - METER_FALL * (now - meter[i][1])
            if db >= current:
                meter[i] = (db, now)
        self._meter = meter


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


def _find_device(found, wanted):
    """
    A device by name: 'in:<name>' is an input, a plain name an output (then
    an input of that name). Exact names first, then a case-insensitive part.
    """
    inputs_only = wanted.startswith('in:')
    name = wanted[3:] if inputs_only else wanted
    candidates = [d for d in found if not d['output']] if inputs_only else \
        sorted(found, key=lambda d: not d['output'])
    for d in candidates:
        if d['name'] == name:
            return d
    lowered = name.lower()
    for d in candidates:
        if lowered in d['name'].lower():
            return d
    return None
