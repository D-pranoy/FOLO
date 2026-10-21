"""
Linux audio capture for the effect engine, on the PulseAudio client library
through cffi (ADR-0031). PipeWire serves the same API through pipewire-pulse.

- Server: its default sink, the sinks and sources, and their change events.
- Record streams: a sink's monitor or a source, 48 kHz float in the device's
  channel count, in packets of about 10 ms. Every read drains everything the
  stream holds, so the analysis always runs on the newest audio.

Each Server runs libpulse's threaded main loop. Its callbacks run on that
loop's thread with the loop locked; they only copy what they get and set
events, and every other call takes the loop's lock. libpulse is loaded when
this module is imported: without it `lib` is None and `MISSING` says so.
"""

# cffi's C objects have no type information: their fields and functions are
# looked up at run time
# pyright: reportAttributeAccessIssue=false

import contextlib
import threading
import time
from typing import Any

import cffi
import numpy as np

ffi = cffi.FFI()
# Only the leading fields FOLO reads are declared: libpulse hands these structs
# over by pointer and never takes one of ours, so their full size never matters.
ffi.cdef(r"""
typedef struct pa_threaded_mainloop pa_threaded_mainloop;
typedef struct pa_mainloop_api pa_mainloop_api;
typedef struct pa_context pa_context;
typedef struct pa_stream pa_stream;
typedef struct pa_operation pa_operation;
typedef uint64_t pa_usec_t;

typedef struct { int format; uint32_t rate; uint8_t channels; } pa_sample_spec;
typedef struct { uint8_t channels; int map[32]; } pa_channel_map;
typedef struct { uint8_t channels; uint32_t values[32]; } pa_cvolume;
typedef struct { uint32_t maxlength; uint32_t tlength; uint32_t prebuf; uint32_t minreq; uint32_t fragsize; } pa_buffer_attr;

typedef struct {
    const char *user_name;
    const char *host_name;
    const char *server_version;
    const char *server_name;
    pa_sample_spec sample_spec;
    const char *default_sink_name;
    const char *default_source_name;
} pa_server_info;

typedef struct {
    const char *name;
    uint32_t index;
    const char *description;
    pa_sample_spec sample_spec;
    pa_channel_map channel_map;
    uint32_t owner_module;
    pa_cvolume volume;
    int mute;
    uint32_t monitor_source;
    const char *monitor_source_name;
} pa_sink_info;

typedef struct {
    const char *name;
    uint32_t index;
    const char *description;
    pa_sample_spec sample_spec;
    pa_channel_map channel_map;
    uint32_t owner_module;
    pa_cvolume volume;
    int mute;
    uint32_t monitor_of_sink;
} pa_source_info;

pa_threaded_mainloop *pa_threaded_mainloop_new(void);
void pa_threaded_mainloop_free(pa_threaded_mainloop *m);
int pa_threaded_mainloop_start(pa_threaded_mainloop *m);
void pa_threaded_mainloop_stop(pa_threaded_mainloop *m);
void pa_threaded_mainloop_lock(pa_threaded_mainloop *m);
void pa_threaded_mainloop_unlock(pa_threaded_mainloop *m);
pa_mainloop_api *pa_threaded_mainloop_get_api(pa_threaded_mainloop *m);

typedef void (*pa_context_notify_cb_t)(pa_context *c, void *userdata);
typedef void (*pa_context_success_cb_t)(pa_context *c, int success, void *userdata);
typedef void (*pa_context_subscribe_cb_t)(pa_context *c, int t, uint32_t idx, void *userdata);
typedef void (*pa_server_info_cb_t)(pa_context *c, const pa_server_info *i, void *userdata);
typedef void (*pa_sink_info_cb_t)(pa_context *c, const pa_sink_info *i, int eol, void *userdata);
typedef void (*pa_source_info_cb_t)(pa_context *c, const pa_source_info *i, int eol, void *userdata);

pa_context *pa_context_new(pa_mainloop_api *mainloop, const char *name);
void pa_context_unref(pa_context *c);
void pa_context_set_state_callback(pa_context *c, pa_context_notify_cb_t cb, void *userdata);
void pa_context_set_subscribe_callback(pa_context *c, pa_context_subscribe_cb_t cb, void *userdata);
int pa_context_connect(pa_context *c, const char *server, int flags, const void *api);
void pa_context_disconnect(pa_context *c);
int pa_context_get_state(const pa_context *c);
int pa_context_errno(const pa_context *c);
const char *pa_strerror(int error);
pa_operation *pa_context_subscribe(pa_context *c, uint32_t m, pa_context_success_cb_t cb, void *userdata);
pa_operation *pa_context_get_server_info(pa_context *c, pa_server_info_cb_t cb, void *userdata);
pa_operation *pa_context_get_sink_info_list(pa_context *c, pa_sink_info_cb_t cb, void *userdata);
pa_operation *pa_context_get_source_info_list(pa_context *c, pa_source_info_cb_t cb, void *userdata);
int pa_operation_get_state(const pa_operation *o);
void pa_operation_cancel(pa_operation *o);
void pa_operation_unref(pa_operation *o);

typedef void (*pa_stream_notify_cb_t)(pa_stream *p, void *userdata);
typedef void (*pa_stream_request_cb_t)(pa_stream *p, size_t nbytes, void *userdata);

pa_stream *pa_stream_new(pa_context *c, const char *name, const pa_sample_spec *ss, const pa_channel_map *map);
void pa_stream_unref(pa_stream *s);
void pa_stream_set_state_callback(pa_stream *s, pa_stream_notify_cb_t cb, void *userdata);
void pa_stream_set_read_callback(pa_stream *p, pa_stream_request_cb_t cb, void *userdata);
void pa_stream_set_overflow_callback(pa_stream *p, pa_stream_notify_cb_t cb, void *userdata);
int pa_stream_connect_record(pa_stream *s, const char *dev, const pa_buffer_attr *attr, int flags);
int pa_stream_disconnect(pa_stream *s);
int pa_stream_get_state(const pa_stream *p);
int pa_stream_peek(pa_stream *p, const void **data, size_t *nbytes);
int pa_stream_drop(pa_stream *p);
int pa_stream_get_latency(pa_stream *s, pa_usec_t *r_usec, int *negative);
""")

lib: Any
try:
    lib = ffi.dlopen('libpulse.so.0')
    MISSING = ''
except OSError:
    lib = None
    MISSING = 'Audio capture needs the PulseAudio client library (libpulse)'

PA_SAMPLE_FLOAT32LE = 5
PA_CONTEXT_NOAUTOSPAWN = 0x1
# pa_context_state_t
PA_CONTEXT_READY = 4
PA_CONTEXT_FAILED = 5
PA_CONTEXT_TERMINATED = 6
# pa_stream_state_t
PA_STREAM_READY = 2
PA_STREAM_FAILED = 3
PA_STREAM_TERMINATED = 4
PA_OPERATION_RUNNING = 0
# pa_stream_flags_t
PA_STREAM_INTERPOLATE_TIMING = 0x2
PA_STREAM_AUTO_TIMING_UPDATE = 0x8
PA_STREAM_DONT_MOVE = 0x200
PA_STREAM_ADJUST_LATENCY = 0x2000
# pa_subscription_mask_t
PA_SUBSCRIPTION_MASK_SINK = 0x1
PA_SUBSCRIPTION_MASK_SOURCE = 0x2
PA_SUBSCRIPTION_MASK_SERVER = 0x80
PA_INVALID_INDEX = 0xFFFFFFFF
NO_VALUE = 0xFFFFFFFF  # (uint32_t) -1: let the server pick this buffer value

CLIENT_NAME = b'FOLO'
TIMEOUT = 5.0  # seconds to wait for the server to connect or answer
PACKET = 0.01  # seconds of audio the server sends at a time


class PulseError(RuntimeError):
    """A call to the audio server failed. `lost`: the device went away."""

    def __init__(self, message, lost=False):
        RuntimeError.__init__(self, message)
        self.lost = lost


def _text(pointer):
    return ffi.string(pointer).decode('utf-8', 'replace') if pointer != ffi.NULL else ''


def _positions(channel_map):
    """A device's channel layout: the position of each channel (pa_channel_position_t)."""
    return tuple(int(channel_map.map[i]) for i in range(channel_map.channels))


class Server:
    """A connection to the audio server, with its own threaded main loop."""

    def __init__(self):
        if lib is None:
            raise PulseError(MISSING)
        self.changes = 0  # server, sink and source events seen since subscribe()
        self._changed = threading.Event()  # the context's state changed
        self._callbacks = []  # kept alive while libpulse may call them
        self._loop = lib.pa_threaded_mainloop_new()
        api = lib.pa_threaded_mainloop_get_api(self._loop)
        self._context = lib.pa_context_new(api, CLIENT_NAME)
        try:
            on_state = self._keep('pa_context_notify_cb_t', lambda c, u: self._changed.set())
            lib.pa_context_set_state_callback(self._context, on_state, ffi.NULL)
            if (
                lib.pa_context_connect(self._context, ffi.NULL, PA_CONTEXT_NOAUTOSPAWN, ffi.NULL)
                < 0
            ):
                raise PulseError(self._error('Connecting to the audio server'))
            if lib.pa_threaded_mainloop_start(self._loop) < 0:
                raise PulseError('The audio server main loop did not start')
            deadline = time.monotonic() + TIMEOUT
            while (state := self._state()) != PA_CONTEXT_READY:
                if state in (PA_CONTEXT_FAILED, PA_CONTEXT_TERMINATED):
                    raise PulseError(self._error('Connecting to the audio server'))
                if time.monotonic() > deadline:
                    raise PulseError('The audio server did not answer')
                self._changed.wait(0.1)
                self._changed.clear()
        except BaseException:
            self.close()
            raise

    # -------------------------------------------------------------- plumbing

    def _keep(self, kind, function):
        callback = ffi.callback(kind, function)
        self._callbacks.append(callback)
        return callback

    @contextlib.contextmanager
    def locked(self):
        lib.pa_threaded_mainloop_lock(self._loop)
        try:
            yield
        finally:
            lib.pa_threaded_mainloop_unlock(self._loop)

    def _state(self):
        with self.locked():
            return lib.pa_context_get_state(self._context)

    def _error(self, what):
        code = lib.pa_context_errno(self._context)
        return f'{what} failed ({_text(lib.pa_strerror(code))})'

    @property
    def connected(self):
        return self._state() == PA_CONTEXT_READY

    def _request(self, what, start, kind, on_item=None):
        """
        Runs one operation and waits for it: `start(callback)` begins it with
        a callback of type `kind`, and `on_item(info)` gets each result. A
        list's callback gets an `eol` flag after the info; the others answer
        once, with info (NULL when it failed) or a success flag.
        """
        done = threading.Event()
        failed = []

        def finish(ok):
            if not ok:
                failed.append(True)
            done.set()

        def listed(context, info, eol, userdata):
            if eol == 0 and info != ffi.NULL and on_item is not None:
                on_item(info)
            elif eol != 0:
                finish(eol > 0)

        def succeeded(context, success, userdata):
            finish(bool(success))

        def answered(context, info, userdata):
            if info != ffi.NULL and on_item is not None:
                on_item(info)
            finish(info != ffi.NULL)

        if kind in ('pa_sink_info_cb_t', 'pa_source_info_cb_t'):
            answer = listed
        elif kind == 'pa_context_success_cb_t':
            answer = succeeded
        else:
            answer = answered
        callback = ffi.callback(kind, answer)
        with self.locked():
            operation = start(callback)
            if operation == ffi.NULL:
                raise PulseError(self._error(what))
        try:
            deadline = time.monotonic() + TIMEOUT
            while not done.wait(0.1):
                if not self.connected:
                    raise PulseError(f'{what} failed (the audio server went away)')
                if time.monotonic() > deadline:
                    raise PulseError(f'{what} failed (the audio server did not answer)')
        finally:
            with self.locked():
                if lib.pa_operation_get_state(operation) == PA_OPERATION_RUNNING:
                    lib.pa_operation_cancel(operation)
                lib.pa_operation_unref(operation)
        if failed:
            raise PulseError(self._error(what))

    # -------------------------------------------------------------- queries

    def server_info(self):
        """{'default_sink', 'default_source'}: their names."""
        found = {}

        def item(info):
            found['default_sink'] = _text(info.default_sink_name)
            found['default_source'] = _text(info.default_source_name)

        self._request(
            'Reading the audio server',
            lambda cb: lib.pa_context_get_server_info(self._context, cb, ffi.NULL),
            'pa_server_info_cb_t',
            item,
        )
        return found

    def sinks(self):
        """The outputs: [{'name', 'description', 'channels', 'positions', 'monitor'}]."""
        found = []

        def item(info):
            found.append(
                {
                    'name': _text(info.name),
                    'description': _text(info.description) or _text(info.name),
                    'channels': int(info.sample_spec.channels),
                    'positions': _positions(info.channel_map),
                    'monitor': _text(info.monitor_source_name),
                }
            )

        self._request(
            'Listing the outputs',
            lambda cb: lib.pa_context_get_sink_info_list(self._context, cb, ffi.NULL),
            'pa_sink_info_cb_t',
            item,
        )
        return found

    def sources(self):
        """
        The inputs, without the outputs' monitors:
        [{'name', 'description', 'channels', 'positions'}].
        """
        found = []

        def item(info):
            if info.monitor_of_sink != PA_INVALID_INDEX:
                return
            found.append(
                {
                    'name': _text(info.name),
                    'description': _text(info.description) or _text(info.name),
                    'channels': int(info.sample_spec.channels),
                    'positions': _positions(info.channel_map),
                }
            )

        self._request(
            'Listing the inputs',
            lambda cb: lib.pa_context_get_source_info_list(self._context, cb, ffi.NULL),
            'pa_source_info_cb_t',
            item,
        )
        return found

    def subscribe(self):
        """Counts server, sink and source events in `changes` from now on."""

        def event(context, kind, index, userdata):
            self.changes += 1

        on_event = self._keep('pa_context_subscribe_cb_t', event)
        mask = PA_SUBSCRIPTION_MASK_SINK | PA_SUBSCRIPTION_MASK_SOURCE | PA_SUBSCRIPTION_MASK_SERVER
        with self.locked():
            lib.pa_context_set_subscribe_callback(self._context, on_event, ffi.NULL)
        self._request(
            'Subscribing to audio server events',
            lambda cb: lib.pa_context_subscribe(self._context, mask, cb, ffi.NULL),
            'pa_context_success_cb_t',
        )

    def record(self, device, channels, rate=48000, positions=None):
        """
        A record stream from a source (a sink's monitor is a source too), in
        the device's channel layout when `positions` gives it.
        """
        return Record(self, device, channels, rate, positions)

    def close(self):
        """Disconnects and stops the main loop. Safe to call more than once."""
        if self._loop is None:
            return
        if self._context is not None:
            with self.locked():
                lib.pa_context_set_state_callback(self._context, ffi.NULL, ffi.NULL)
                lib.pa_context_set_subscribe_callback(self._context, ffi.NULL, ffi.NULL)
                lib.pa_context_disconnect(self._context)
                lib.pa_context_unref(self._context)
            self._context = None
        lib.pa_threaded_mainloop_stop(self._loop)
        lib.pa_threaded_mainloop_free(self._loop)
        self._loop = None
        self._callbacks = []


class Record:
    """
    A record stream at `rate` float32 in `channels`, laid out as `positions`
    (else libpulse's default layout for that many channels). The server
    signals each new packet; wait() sleeps until one is there and read()
    takes them all.
    """

    def __init__(self, server, device, channels, rate, positions=None):
        self.server = server
        self.rate = rate
        self.channels = channels
        self.period = PACKET
        self.event_driven = True
        self.overflows = 0  # audio the server dropped because FOLO read too late
        self._ready = threading.Event()  # audio waits to be read
        self._changed = threading.Event()  # the stream's state changed
        self._callbacks = []

        def overflow(stream, userdata):
            self.overflows += 1

        spec = ffi.new(
            'pa_sample_spec *', {'format': PA_SAMPLE_FLOAT32LE, 'rate': rate, 'channels': channels}
        )
        layout = ffi.NULL
        if positions and len(positions) == channels:
            layout = ffi.new('pa_channel_map *')
            layout.channels = channels
            for i, position in enumerate(positions):
                layout.map[i] = position
        attr = ffi.new(
            'pa_buffer_attr *',
            {
                'maxlength': NO_VALUE,
                'tlength': NO_VALUE,
                'prebuf': NO_VALUE,
                'minreq': NO_VALUE,
                'fragsize': round(rate * PACKET) * channels * 4,
            },
        )
        flags = (
            PA_STREAM_ADJUST_LATENCY
            | PA_STREAM_AUTO_TIMING_UPDATE
            | PA_STREAM_INTERPOLATE_TIMING
            | PA_STREAM_DONT_MOVE
        )
        with server.locked():
            self._stream = lib.pa_stream_new(server._context, b'FOLO capture', spec, layout)
            if self._stream == ffi.NULL:
                self._stream = None
                raise PulseError(server._error('Opening the audio stream'))
            on_state = self._keep('pa_stream_notify_cb_t', lambda s, u: self._changed.set())
            on_read = self._keep('pa_stream_request_cb_t', lambda s, n, u: self._ready.set())
            on_overflow = self._keep('pa_stream_notify_cb_t', overflow)
            lib.pa_stream_set_state_callback(self._stream, on_state, ffi.NULL)
            lib.pa_stream_set_read_callback(self._stream, on_read, ffi.NULL)
            lib.pa_stream_set_overflow_callback(self._stream, on_overflow, ffi.NULL)
            connected = lib.pa_stream_connect_record(self._stream, device.encode(), attr, flags)
        try:
            if connected < 0:
                raise PulseError(server._error('Opening the audio stream'))
            deadline = time.monotonic() + TIMEOUT
            while (state := self._state()) != PA_STREAM_READY:
                if state in (PA_STREAM_FAILED, PA_STREAM_TERMINATED):
                    raise PulseError(server._error('Opening the audio stream'))
                if time.monotonic() > deadline:
                    raise PulseError('The audio stream did not start')
                self._changed.wait(0.1)
                self._changed.clear()
        except BaseException:
            self.close()
            raise

    def _keep(self, kind, function):
        callback = ffi.callback(kind, function)
        self._callbacks.append(callback)
        return callback

    def _state(self):
        with self.server.locked():
            return lib.pa_stream_get_state(self._stream)

    def wait(self, timeout):
        self._ready.wait(timeout)
        self._ready.clear()

    def read(self):
        """
        Everything the stream holds: (frames x channels float32, or None when
        there's nothing new, and the stream's latency in ms). Raises
        PulseError once the stream has failed (its device went away).
        """
        chunks = []
        latency = 0.0
        data = ffi.new('const void **')
        size = ffi.new('size_t *')
        usec = ffi.new('pa_usec_t *')
        negative = ffi.new('int *')
        with self.server.locked():
            state = lib.pa_stream_get_state(self._stream)
            if state != PA_STREAM_READY:
                raise PulseError('The device was disconnected', lost=True)
            while True:
                if lib.pa_stream_peek(self._stream, data, size) < 0:
                    raise PulseError(self.server._error('Reading the audio stream'))
                if size[0] == 0:
                    break
                if data[0] != ffi.NULL:  # NULL: a hole in the stream, nothing to read
                    chunks.append(bytes(ffi.buffer(data[0], size[0])))
                lib.pa_stream_drop(self._stream)
            if chunks and lib.pa_stream_get_latency(self._stream, usec, negative) == 0:
                latency = 0.0 if negative[0] else usec[0] / 1000.0
        if not chunks:
            return None, latency
        samples = np.frombuffer(b''.join(chunks), dtype=np.float32)
        usable = len(samples) - len(samples) % self.channels
        return samples[:usable].reshape(-1, self.channels), latency

    def close(self):
        """Closes the stream and the server connection it came from. Safe to call more than once."""
        if self._stream is not None:
            with self.server.locked():
                lib.pa_stream_set_state_callback(self._stream, ffi.NULL, ffi.NULL)
                lib.pa_stream_set_read_callback(self._stream, ffi.NULL, ffi.NULL)
                lib.pa_stream_set_overflow_callback(self._stream, ffi.NULL, ffi.NULL)
                lib.pa_stream_disconnect(self._stream)
                lib.pa_stream_unref(self._stream)
            self._stream = None
        self.server.close()
        self._callbacks = []
