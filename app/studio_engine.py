"""
Engine of the FOLO studio: captures what is playing, analyses the spectrum,
renders the configured segments to led colors and sends them to hyperion.
"""

import collections
import math
import threading
import time
from collections.abc import Sequence
from typing import Any

import numpy as np
import numpy.typing as npt

from app import audio_input, beat_tracker, now_playing
from app.json_client import JsonClient

SAMPLERATE = 48000
FFT_SIZE = 4096  # spectrum and level triggers: 11.7 Hz bins, 85 ms window
FAST_FFT_SIZE = 2048  # hits triggers: half the window, so hits light 20-30 ms sooner
# Samples kept per channel. New audio is appended, and only when the buffer is
# full do the latest FFT_SIZE samples move back to the front.
HISTORY = 2 * FFT_SIZE
MAX_DT = 0.1  # longest step the renderer takes after a gap in the audio
SILENCE_DB = -120.0
# A channel takes part in the analysis once it gets louder than this. The
# unused channels of the 5.1 output aren't digital silence, they hiss at -190 dB.
ACTIVE_LEVEL = 10.0 ** (-100.0 / 20.0)
# The dB scale the presets were tuned on. The engine used to average all six
# channels of the 5.1 output, four of them silent, which reads 9.5 dB below
# the level of the stereo pair. It now averages the power of the channels
# that carry sound and keeps that scale, whatever the channel layout.
CALIBRATION_DB = -20.0 * math.log10(3.0)
# A windowed sum of samples no louder than `peak` is at most peak * sum(window),
# so no bin reads above 20 log10(peak) + 6 dB + CALIBRATION_DB + the input gain.
# While the analysis window stays below that for SILENCE_DB (1 dB of margin),
# every bin would read SILENCE_DB and the FFTs are skipped.
QUIET_PEAK_DB = SILENCE_DB - 1.0 - 20.0 * math.log10(2.0) - CALIBRATION_DB

DISPLAY_BANDS = 192
DISPLAY_MIN_HZ = 20.0
DISPLAY_MAX_HZ = 20000.0

AVERAGE_MS = 1000  # "recent average" that hits are measured against
PEAK_HOLD = 0.5  # seconds the peak marker of fill mode holds
PEAK_FALL = 0.8  # peak marker fall speed in segment lengths/second

# Moving modes ported from LedFx effects. They draw colors per step
# themselves; drops and wipe fire on hits (the level jumping past HIT_ON
# after falling below HIT_OFF).
MOTION_MODES = ('scroll', 'drops', 'scan', 'wipe', 'flow', 'fire')
HIT_ON = 0.5
HIT_OFF = 0.25
HIT_GAP = 0.06  # shortest time between two hits of a segment
MAX_DROPS = 32

# Beat trigger: a segment flashes on the beats of beat_tracker, as bright as
# its band has been lately (its level, holding its peaks for about a beat)
BEAT_PULSE = 0.04  # seconds a beat holds full brightness at least, so a frame sent shows it
BEAT_HOLD_FALL = 12.0  # dB per second the held level falls
# The beat tracker runs while a segment uses it, and this many seconds after
# a page last streamed (they show the tempo)
BEAT_WATCH = 3.0

# Ambient light (config 'reactive' and 'ambient'): what the leds show when
# audio reactiveness is off, or in auto mode after a stretch of silence
QUIET_DB = -70.0  # audio counts as silent while its loudest bin is below this
AMBIENT_FADE_IN = 1.5  # seconds to fade into the ambient light
AMBIENT_FADE_OUT = 0.15  # seconds back to the reactive look, so the first beat shows
BREATHE_PERIOD = 8.0  # seconds per breath of the breathe style
BREATHE_LOW = 0.3  # breathe style: dimmest point, as a share of the color
# With now_playing.pause_ambient, auto mode switches as soon as the player is
# paused and the audio has been quiet this long
PAUSE_QUIET = 0.3
# Without audio packets (nothing playing on some devices, or no device) the
# engine still renders, so the ambient light fades in and keeps going
IDLE_AFTER = 0.1  # seconds without a packet before idle rendering starts
IDLE_FPS = 30

# Auto level (config audio.auto_level): follows how loud the music's loud
# moments are and adds gain so they sit where the presets were tuned (the hot
# output of the original setup). Silence holds the gain where it was.
AUTO_TARGET_DB = -15.0  # loudest bin of loud moments, after the gain
AUTO_MIN_DB = -12.0
AUTO_MAX_DB = 30.0
AUTO_RISE_MS = 300  # how fast it follows louder music
AUTO_FALL_MS = 8000  # and quieter music

# Hyperion drops our colors after this many ms without updates, so the leds
# are released even if the studio is killed
HYPERION_DURATION = 2000
# An unchanged frame (silence, a solid ambient light) is sent again only this
# often, well within HYPERION_DURATION and WLED's live mode timeout (2.5 s)
KEEPALIVE = 0.5
RECONNECT_DELAY = 5.0


def audio_sources():
    """
    The default output, the devices and the apps playing audio (see
    audio_input.sources).
    """
    return audio_input.sources()


def hex_to_rgb(color):
    return (int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16))


_palettes = {}


def palette_colors(palette):
    """
    A palette's colors as a (colors x rgb) array, read-only and shared.
    """
    key = tuple(palette)
    colors = _palettes.get(key)
    if colors is None:
        if len(_palettes) > 256:
            _palettes.clear()
        colors = np.array([hex_to_rgb(c) for c in palette], dtype=np.float64)
        colors.flags.writeable = False
        _palettes[key] = colors
    return colors


def shown_levels(levels: dict[str, tuple[float, float, float]]) -> dict[str, list[float]]:
    """
    Renderer.render's segment levels as the web UI gets them:
    {segment id: [level dB, value, recent average dB]}, rounded.
    """
    return {
        seg_id: [round(level, 1), round(value, 3), round(average, 1)]
        for seg_id, (level, value, average) in levels.items()
    }


def smoothing(ms, dt):
    """
    Share of the distance to the target covered in dt seconds.
    """
    if ms <= 0:
        return 1.0
    return 1.0 - math.exp(-dt * 1000.0 / ms)


class Spectrum:
    """
    dB levels of the latest samples: `full` from the FFT_SIZE window (the
    spectrum the UI shows, level triggers), `fast` from the FAST_FFT_SIZE one
    (hits triggers, worked out the first time something asks). Built from
    arrays with one SILENCE_DB bin of padding at the top, which band_levels()
    needs.
    """

    __slots__ = ('_fast', '_loudest', 'full', 'offset', 'padded')

    def __init__(self, full_padded, fast, offset=0.0):
        self.full = full_padded[:-1]
        self.padded = {False: full_padded}
        self._fast = fast  # padded array, or a function returning it
        self.offset = offset  # input gain included in the levels
        self._loudest = None

    @property
    def loudest(self):
        """The loudest bin without the input gain: how loud the source really is."""
        if self._loudest is None:
            self._loudest = float(self.full.max()) - self.offset
        return self._loudest

    def padded_for(self, fast: bool) -> npt.NDArray[np.float64]:
        if fast and fast not in self.padded:
            self.padded[True] = self._fast() if callable(self._fast) else self._fast
        return self.padded[fast]

    @property
    def fast(self):
        return self.padded_for(True)[:-1]

    @classmethod
    def silent(cls):
        return cls(_SILENT_FULL, _SILENT_FAST)


# The arrays of Spectrum.silent(), shared and read-only
_SILENT_FULL = np.full(FFT_SIZE // 2 + 2, SILENCE_DB)
_SILENT_FAST = np.full(FAST_FFT_SIZE // 2 + 2, SILENCE_DB)
_SILENT_FULL.flags.writeable = False
_SILENT_FAST.flags.writeable = False


class Analyzer:
    """
    Spectra of the latest samples in dB on the presets' scale (CALIBRATION_DB;
    a full scale sine on both stereo channels is -9.5 dB). The power of every
    channel that carries sound is averaged, so wide stereo and out of phase
    sounds count fully and silent surround channels don't dilute the rest.
    """

    def __init__(self):
        self.freqs = np.fft.rfftfreq(FFT_SIZE, 1.0 / SAMPLERATE)
        self.fast_freqs = np.fft.rfftfreq(FAST_FFT_SIZE, 1.0 / SAMPLERATE)
        self.display_edges = np.geomspace(DISPLAY_MIN_HZ, DISPLAY_MAX_HZ, DISPLAY_BANDS + 1)
        # channels x HISTORY samples, the latest FFT_SIZE end at self.end
        self.history = np.zeros((0, HISTORY))
        self.end = FFT_SIZE
        self.active = np.zeros(0, dtype=bool)
        self._playing = np.zeros(0, dtype=np.intp)  # indices of the active channels
        # (frames, loudest sample of the active channels) of the latest
        # packets, as far back as the window reaches
        self._recent = collections.deque()
        self._recent_frames = 0
        self._forget_recent()
        self.offset = 0.0  # input gain in dB, added to every level
        self._windows = {}
        for size in (FFT_SIZE, FAST_FFT_SIZE):
            window = np.hanning(size)
            self._windows[size] = (window, (2.0 / window.sum()) ** 2)
        self._plans = {}

    @property
    def samples(self):
        """The latest FFT_SIZE samples of every channel (a view)."""
        return self.history[:, self.end - FFT_SIZE : self.end]

    def clear(self):
        """
        Forgets the samples, after a gap in the audio: they no longer belong
        to what's playing.
        """
        self.samples[:] = 0.0
        self._forget_recent()

    def _forget_recent(self):
        # A window of zeros
        self._recent.clear()
        self._recent.append((FFT_SIZE, 0.0))
        self._recent_frames = FFT_SIZE

    def _remember(self, count, loudest):
        recent = self._recent
        if count >= FFT_SIZE:
            recent.clear()
            self._recent_frames = 0
        recent.append((count, loudest))
        self._recent_frames += count
        while self._recent_frames - recent[0][0] >= FFT_SIZE:
            self._recent_frames -= recent.popleft()[0]

    def _quiet(self, offset):
        """True while the window is too quiet for any bin to read above SILENCE_DB."""
        limit = 10.0 ** ((QUIET_PEAK_DB - offset) / 20.0)
        return all(loudest <= limit for _, loudest in self._recent)

    def push(
        self, frames: npt.ArrayLike, peaks: npt.NDArray[np.floating] | None = None
    ) -> Spectrum:
        """
        Adds captured frames (samples x channels, or mono) and returns the new
        Spectrum. `peaks`: each channel's peak in the frames, if the caller
        has them already.
        """
        frames = np.asarray(frames)
        if frames.ndim == 1:
            frames = frames[:, None]
        count, channels = frames.shape
        if channels != len(self.active):
            self.history = np.zeros((channels, HISTORY))
            self.end = FFT_SIZE
            self.active = np.zeros(channels, dtype=bool)
            self._playing = np.zeros(0, dtype=np.intp)
            self._forget_recent()
        if count >= FFT_SIZE:
            self.end = FFT_SIZE
            self.history[:, :FFT_SIZE] = frames[-FFT_SIZE:].T
        elif count:
            if self.end + count > HISTORY:
                self.history[:, :FFT_SIZE] = self.samples
                self.end = FFT_SIZE
            self.history[:, self.end : self.end + count] = frames.T
            self.end += count
        # A channel counts from the first time it carries sound
        if count:
            if peaks is None or len(peaks) != channels:
                peaks = audio_input.channel_peaks(frames)
            peaks = np.asarray(peaks)
            active = self.active | (peaks > ACTIVE_LEVEL)
            if (active != self.active).any():
                self.active = active
                self._playing = np.flatnonzero(active)
            # numpy's max, unlike Python's, passes a NaN on, which keeps the FFTs on
            self._remember(count, float(peaks[self._playing].max()) if len(self._playing) else 0.0)
        if not len(self._playing):
            return Spectrum.silent()
        offset = self.offset
        if self._quiet(offset):
            # What the FFTs would return: SILENCE_DB in every bin
            return Spectrum(_SILENT_FULL, _SILENT_FAST, offset)
        playing = self.history[self._playing, self.end - FFT_SIZE : self.end]
        return Spectrum(
            self._db(playing, FFT_SIZE, offset),
            lambda: self._db(playing, FAST_FFT_SIZE, offset),
            offset,
        )

    def _db(self, playing, size, offset):
        window, norm = self._windows[size]
        bins = np.fft.rfft(playing[:, -size:] * window, axis=1)
        # The mean power of the channels, in place to spare temporaries. One
        # or two channels (the usual case) skip the reduction's overhead.
        power = bins.real**2
        power += bins.imag**2
        if len(power) == 1:
            power = power[0]
        elif len(power) == 2:
            power = power[0] + power[1]
        else:
            power = np.add.reduce(power, axis=0)
        power /= len(playing)
        power *= norm
        np.maximum(power, 1e-30, out=power)
        np.log10(power, out=power)
        power *= 10.0
        power += CALIBRATION_DB + offset
        db = np.empty(len(power) + 1)
        db[-1] = SILENCE_DB
        np.maximum(power, SILENCE_DB, out=db[:-1])
        return db

    def band_plan(self, edges, fast=False):
        """
        How band_levels() reads the bins for these edges (Hz), worked out
        once. Pass it to planned_levels().
        """
        edges = np.asarray(edges, dtype=np.float64)
        key = (fast, edges.tobytes())
        plan = self._plans.get(key)
        if plan is None:
            if len(self._plans) > 512:
                self._plans.clear()
            idx = np.searchsorted(self.fast_freqs if fast else self.freqs, edges)
            empty = idx[1:] <= idx[:-1]
            centers = np.sqrt(edges[:-1] * edges[1:])[empty]
            plan = self._plans[key] = (fast, idx, empty, centers, 1)
        return plan

    def bands_plan(self, bands, fast=False):
        """
        Like band_plan(), for separate (lo, hi) bands that may overlap.
        """
        edges = np.asarray(bands, dtype=np.float64).reshape(-1)
        idx = np.searchsorted(self.fast_freqs if fast else self.freqs, edges)
        empty = idx[1::2] <= idx[0::2]
        centers = np.sqrt(edges[0::2] * edges[1::2])[empty]
        return (fast, idx, empty, centers, 2)

    def planned_levels(self, spectrum, plan):
        """
        Loudest bin of each band of a plan, from the short window if the plan
        is `fast`. Bands narrower than the FFT resolution are interpolated.
        """
        fast, idx, empty, centers, step = plan
        padded = spectrum.padded_for(fast)
        # reduceat takes the max of padded[idx[i]:idx[i + 1]] (padded[idx[i]]
        # when that is empty); the padding keeps an index at the top edge valid
        levels = np.maximum.reduceat(padded, idx)
        levels = levels[:-1] if step == 1 else levels[::2]
        if centers.size:
            levels[empty] = np.interp(centers, self.fast_freqs if fast else self.freqs, padded[:-1])
        return levels

    def band_levels(self, spectrum, edges, fast=False):
        """
        Loudest bin between each pair of edges (Hz), from the short window if
        `fast`. Bands narrower than the FFT resolution are interpolated.
        """
        return self.planned_levels(spectrum, self.band_plan(edges, fast))

    def band_level(self, spectrum, freq_lo, freq_hi, fast=False):
        return float(self.band_levels(spectrum, (freq_lo, freq_hi), fast)[0])


class SegmentState:
    def __init__(self):
        self.value = 0.0
        self.peak = 0.0
        self.peak_age = 0.0
        self.bands = None
        self.average = None
        self.shown_average = None  # hits: average of the level the UI's spectrum shows
        self.band_average = None
        # moving modes
        self.armed = True
        self.since_hit = 1.0
        self.buffer = None  # scroll trail colors or fire heat, per step
        self.carry = 0.0  # fraction of a step not moved yet
        self.drops = []  # [center step, age s, rgb, strength]
        self.pos = 0.0  # scan eye start, wipe progress or flow phase
        self.heading = 1.0
        self.color_index = 0
        # beat trigger
        self.held = None  # the band's level, holding its peaks
        self.pulse = 0.0  # seconds the current beat still shows


class AutoLevel:
    """
    Follows how loud the source's loud moments are (an envelope of the
    loudest bin without gain) and works out the gain that puts them at
    AUTO_TARGET_DB. It keeps tracking while switched off, so switching it on
    takes effect at once.
    """

    def __init__(self):
        self.envelope = None
        self.gain = 0.0

    def reset(self):
        self.envelope = None
        self.gain = 0.0

    def update(self, loudest: float, dt: float) -> None:
        if loudest < QUIET_DB:
            return
        if self.envelope is None:
            self.envelope = loudest
        ms = AUTO_RISE_MS if loudest > self.envelope else AUTO_FALL_MS
        self.envelope += (loudest - self.envelope) * smoothing(ms, dt)
        self.gain = min(AUTO_MAX_DB, max(AUTO_MIN_DB, AUTO_TARGET_DB - self.envelope))


class Ambient:
    """
    Crossfades between the reactive frame and the ambient light. `amount` is
    0 for the reactive look and 1 for the ambient light; `quiet` counts the
    seconds the audio has been silent.
    """

    def __init__(self):
        self.amount = 0.0
        self.quiet = 0.0
        self.phase = 0.0

    def wanted(self, config, paused=False):
        """
        1 for the ambient light, 0 for the reactive look. `paused`: the
        player says the music is paused (only counts with pause_ambient).
        """
        mode = config['reactive']
        if mode == 'off':
            return 1.0
        if mode == 'auto':
            idle = config['ambient']['idle_after']
            if paused and config['now_playing']['pause_ambient']:
                idle = min(idle, PAUSE_QUIET)
            if self.quiet >= idle:
                return 1.0
        return 0.0

    def update(self, config, spectrum, dt, paused=False):
        self.quiet = 0.0 if spectrum.loudest >= QUIET_DB else self.quiet + dt
        self.phase = (self.phase + dt / BREATHE_PERIOD) % 1.0
        want = self.wanted(config, paused)
        if want > self.amount:
            self.amount = min(want, self.amount + dt / AMBIENT_FADE_IN)
        elif want < self.amount:
            self.amount = max(want, self.amount - dt / AMBIENT_FADE_OUT)

    def color(self, config):
        ambient = config['ambient']
        rgb = np.array(hex_to_rgb(ambient['color']), dtype=np.float64) * ambient['level']
        if ambient['style'] == 'breathe':
            breath = 0.5 - 0.5 * math.cos(2.0 * math.pi * self.phase)
            rgb *= BREATHE_LOW + (1.0 - BREATHE_LOW) * breath
        return rgb

    def blend(self, out, config, spectrum, dt, paused=False):
        """
        Mixes the ambient light into `out` (leds x rgb, before `master`).
        """
        self.update(config, spectrum, dt, paused)
        if self.amount <= 0.0:
            return out
        a = self.amount * self.amount * (3.0 - 2.0 * self.amount)  # eased
        return out * (1.0 - a) + self.color(config) * a


class _Part:
    """
    What the renderer works out once per config for a segment.
    """

    __slots__ = (
        'bands',
        'beats',
        'colors',
        'fast_index',
        'hits',
        'id',
        'index',
        'pos',
        'seg',
        'start',
        'steps',
        'stop',
        'visible',
    )

    def __init__(self, seg, index):
        self.seg = seg
        self.id = seg['id']
        self.index = index  # in the full window's levels
        self.hits = seg['trigger'] == 'hits'
        # beat trigger: every this many beats
        self.beats = seg['beats'] if seg['trigger'] == 'beat' else 0.0
        self.fast_index: int | None = None  # in the short window's levels (hits)
        self.visible: bool = False
        self.start: int = 0  # leds start:stop
        self.stop: int = 0
        self.pos: npt.NDArray[np.int_] | None = None
        self.steps: int = 0
        # static gradient of fill, pulse and spectrum
        self.colors: npt.NDArray[np.float64] | None = None
        self.bands: Any = None  # band plan of spectrum mode


class Renderer:
    """
    Turns the spectrum into led colors according to the segments, and blends
    in the ambient light.
    """

    def __init__(self, analyzer, rng=None):
        self.analyzer = analyzer
        self.ambient = Ambient()
        self.states = {}
        # Drops and fire are random; tests pass a seeded generator
        self.rng = np.random.default_rng() if rng is None else rng
        self._layouts = {}
        self._gradients = {}
        self._config: dict[str, Any] | None = None
        self._colors: tuple[str, ...] = ()
        self._plan: Any = None

    def _prepare(self, config: dict[str, Any], colors: tuple[str, ...] = ()) -> Any:
        """
        The parts of rendering that only change with the config: band plans,
        layouts and colors. The store replaces the config on every change and
        never edits it, so it's worked out again when the config is a new one,
        or when `colors` (the album art's, replacing the palettes) change.
        """
        if config is self._config and colors == self._colors:
            return self._plan
        count = config['led_count']
        segments = config['segments']
        if colors:
            segments = [dict(seg, palette=cover_palette(seg, colors)) for seg in segments]
        solo = any(s['solo'] and s['enabled'] for s in segments)
        parts = []
        fast_bands = []
        for index, seg in enumerate(segments):
            part = _Part(seg, index)
            if part.hits:
                part.fast_index = len(fast_bands)
                fast_bands.append((seg['freq_lo'], seg['freq_hi']))
            start = seg['start']
            end = min(seg['end'], count - 1)
            part.visible = seg['enabled'] and (seg['solo'] or not solo) and end >= start
            if part.visible:
                size = end - start + 1
                part.start, part.stop = start, end + 1
                pos, steps = self.layout(size, seg['direction'])
                part.pos, part.steps = pos, steps
                mode = seg['mode']
                if mode in ('fill', 'pulse', 'spectrum'):
                    t = pos / float(max(1, steps - 1))
                    part.colors = self.static_gradient(seg['palette'], size, seg['direction'], t)
                if mode == 'spectrum':
                    edges = np.geomspace(seg['freq_lo'], seg['freq_hi'], part.steps + 1)
                    part.bands = self.analyzer.band_plan(edges, fast=part.hits)
            parts.append(part)
        background = np.array(hex_to_rgb(config['background']), dtype=np.float64)
        base = np.tile(background, (count, 1))
        full = (
            self.analyzer.bands_plan([(s['freq_lo'], s['freq_hi']) for s in segments])
            if segments
            else None
        )
        fast = self.analyzer.bands_plan(fast_bands, fast=True) if fast_bands else None
        self._config = config
        self._colors = colors
        self._plan = (base, parts, full, fast)
        return self._plan

    def layout(self, size, direction):
        """
        Step index of every led of a segment along its direction, and the
        number of steps (mirrored directions have half as many).
        """
        key = (size, direction)
        if key not in self._layouts:
            k = np.arange(size)
            if direction == 'reverse':
                pos = size - 1 - k
            elif direction in ('center', 'edges'):
                pos = np.floor(np.abs(k - (size - 1) / 2.0)).astype(int)
                if direction == 'edges':
                    pos = pos.max() - pos
            else:
                pos = k
            self._layouts[key] = (pos, int(pos.max()) + 1)
        return self._layouts[key]

    @staticmethod
    def gradient(palette, t):
        colors = palette_colors(palette)
        if len(colors) == 1:
            return np.repeat(colors, len(t), axis=0)
        pos = np.clip(t, 0.0, 1.0) * (len(colors) - 1)
        i = np.minimum(pos.astype(int), len(colors) - 2)
        f = (pos - i)[:, None]
        return colors[i] * (1.0 - f) + colors[i + 1] * f

    @staticmethod
    def color_at(palette, t):
        """
        gradient() at a single position, worked out in plain Python up to the
        color itself.
        """
        colors = palette_colors(palette)
        if len(colors) == 1:
            return colors[0].copy()
        pos = (0.0 if t < 0.0 else 1.0 if t > 1.0 else t) * (len(colors) - 1)
        i = min(int(pos), len(colors) - 2)
        f = pos - i
        return colors[i] * (1.0 - f) + colors[i + 1] * f

    def static_gradient(self, palette, size, direction, t):
        """
        gradient() cached by palette, size and direction. `t` may be a
        function returning it, called only when the colors aren't cached.
        """
        key = (tuple(palette), size, direction)
        colors = self._gradients.get(key)
        if colors is None:
            if len(self._gradients) > 256:
                self._gradients.clear()
            colors = self._gradients[key] = self.gradient(palette, t() if callable(t) else t)
        return colors

    def step_gradient(self, palette, steps):
        return self.static_gradient(palette, steps, 'steps', lambda: np.linspace(0.0, 1.0, steps))

    @staticmethod
    def shape(level, seg):
        """
        Maps a level in dB to 0-1: threshold is 0, ceiling is 1. For hits the
        level is dB above the recent average.
        """
        span = seg['ceiling'] - seg['threshold']
        value = np.clip((level - seg['threshold']) / span * seg['gain'], 0.0, 1.0)
        return value ** seg['curve']

    @staticmethod
    def shape_one(level, seg):
        """
        shape() of a single level, in plain Python: numpy takes microseconds
        per call even on one number.
        """
        value = (level - seg['threshold']) / (seg['ceiling'] - seg['threshold']) * seg['gain']
        value = 0.0 if value < 0.0 else 1.0 if value > 1.0 else value
        return value ** seg['curve']

    def _beat_target(self, state, seg, raw, every, beat, dt):
        """
        Beat trigger: the band's held level, shaped like a level trigger,
        for a moment after each beat of the grid (`every` beats), else 0.
        The moment lasts long enough for the attack to bring it up.
        """
        if state.held is None or raw > state.held:
            state.held = raw
        else:
            state.held -= BEAT_HOLD_FALL * dt
        if beat is not None and beat.fired(every):
            state.pulse = max(BEAT_PULSE, 3.0 * seg['attack'] / 1000.0)
        if state.pulse <= 0.0:
            return 0.0
        state.pulse -= dt
        return self.shape_one(state.held, seg)

    @staticmethod
    def _update_peak(state, dt):
        if state.value >= state.peak:
            state.peak = state.value
            state.peak_age = 0.0
        else:
            state.peak_age += dt
            if state.peak_age > PEAK_HOLD:
                state.peak = max(state.value, state.peak - PEAK_FALL * dt)

    def _spectrum(self, state, seg, spectrum, part, dt):
        steps = part.steps
        levels = self.analyzer.planned_levels(spectrum, part.bands)
        if part.hits:
            if state.band_average is None or len(state.band_average) != steps:
                state.band_average = levels.copy()
            state.band_average += (levels - state.band_average) * smoothing(AVERAGE_MS, dt)
            levels = levels - state.band_average
        target = self.shape(levels, seg)
        if state.bands is None or len(state.bands) != steps:
            state.bands = np.zeros(steps)
        rate = np.where(
            target > state.bands, smoothing(seg['attack'], dt), smoothing(seg['release'], dt)
        )
        state.bands += (target - state.bands) * rate
        return state.bands

    @staticmethod
    def _hit(state, target, dt):
        """
        True when the level jumps past HIT_ON after having dropped below
        HIT_OFF, so one hit fires once.
        """
        state.since_hit += dt
        if state.armed and target >= HIT_ON and state.since_hit >= HIT_GAP:
            state.armed = False
            state.since_hit = 0.0
            return True
        if target < HIT_OFF:
            state.armed = True
        return False

    @staticmethod
    def _buffer(state, shape):
        if state.buffer is None or state.buffer.shape != shape:
            state.buffer = np.zeros(shape)
            state.carry = 0.0
        return state.buffer

    @staticmethod
    def _advance(state, speed, dt):
        """
        Whole steps to move this frame at speed steps/second.
        """
        state.carry += speed * dt
        moved = int(state.carry)
        state.carry -= moved
        return moved

    def _scroll(self, state, seg, steps, dt):
        """
        LedFx Scroll: the color of the moment streams away from the start
        and fades out over `width` percent of the segment.
        """
        trail = self._buffer(state, (steps, 3))
        moved = min(steps, self._advance(state, seg['speed'], dt))
        if moved:
            trail[moved:] = trail[:-moved]
        length = max(1.0, steps * seg['width'] / 100.0)
        fade = length / max(1.0, seg['speed']) / 3.0  # ~5% left at the end
        trail *= math.exp(-dt / fade)
        color = self.color_at(seg['palette'], state.value) * state.value
        if moved:
            trail[:moved] = color
        else:
            np.maximum(trail[0], color, out=trail[0])
        return np.maximum(trail, self.step_gradient(seg['palette'], steps) * seg['floor'])

    def _drops(self, state, seg, steps, dt, hit, target):
        """
        LedFx Rain and Spotlight: every hit drops a ripple at a random spot.
        It grows at `speed`, is `width` percent wide and fades with release.
        """
        if hit and len(state.drops) < MAX_DROPS:
            color = self.color_at(seg['palette'], float(self.rng.random(1)[0]))
            state.drops.append([self.rng.uniform(0, steps - 1), 0.0, color, target])
        k = np.arange(steps)
        half = max(0.75, steps * seg['width'] / 200.0)
        life = max(20.0, seg['release']) / 1000.0
        rgb = self.step_gradient(seg['palette'], steps) * seg['floor']
        alive = []
        for drop in state.drops:
            center, age, color, strength = drop
            drop[1] = age = age + dt
            radius = seg['speed'] * age
            fade = strength * math.exp(-age / life)
            if fade < 0.02 or radius - half > steps:
                continue
            alive.append(drop)
            ring = np.clip(1.0 - np.abs(np.abs(k - center) - radius) / half, 0.0, 1.0)
            np.maximum(rgb, color * (ring * fade)[:, None], out=rgb)
        state.drops = alive
        return rgb

    def _scan(self, state, seg, steps, dt):
        """
        LedFx Scan: an eye `width` percent long bounces between the ends,
        moving faster the louder the band is.
        """
        length = min(float(steps), max(1.0, steps * seg['width'] / 100.0))
        travel = steps - length
        state.pos += state.heading * seg['speed'] * state.value * dt
        if state.pos >= travel:
            state.pos, state.heading = travel, -1.0
        elif state.pos <= 0.0:
            state.pos, state.heading = 0.0, 1.0
        k = np.arange(steps) + 0.5
        inside = np.minimum(k - state.pos, state.pos + length - k)
        soft = max(0.5, min(2.0, length / 4.0))
        eye = np.clip(inside / soft + 0.5, 0.0, 1.0)
        color = self.color_at(seg['palette'], (state.pos + length / 2.0) / steps)
        level = seg['floor'] + (1.0 - seg['floor']) * state.value
        return color * (eye * level)[:, None]

    def _wipe(self, state, seg, steps, dt, hit):
        """
        LedFx Multicolor Bar and Bar: every hit sweeps the next palette color
        across, turning around each time. Below 100% `width` only a bar that
        long travels.
        """
        palette = seg['palette']
        if hit:
            state.color_index = (state.color_index + 1) % len(palette)
            state.heading = -state.heading
            state.pos = 0.0
        state.pos = min(1.0, max(0.0, state.pos) + seg['speed'] * dt / steps)
        front = (1.0 - (1.0 - state.pos) ** 2) * steps  # eases out like LedFx
        k = np.arange(steps) + 0.5
        s = k if state.heading > 0 else steps - k
        behind = np.clip(front - s + 0.5, 0.0, 1.0)
        colors = palette_colors(palette)
        new = colors[state.color_index % len(palette)]
        if seg['width'] >= 100:
            old = colors[(state.color_index - 1) % len(palette)]
            rgb = new * behind[:, None] + old * (1.0 - behind)[:, None]
        else:
            length = max(1.0, steps * seg['width'] / 100.0)
            rgb = new * (behind * np.clip(s - (front - length) + 0.5, 0.0, 1.0))[:, None]
        return rgb * (seg['floor'] + (1.0 - seg['floor']) * state.value)

    def _flow(self, state, seg, steps, dt):
        """
        LedFx Lava lamp and Marching: palette blobs `width` percent long
        drift along at `speed`, pushed faster and brighter by the band.
        """
        state.pos += seg['speed'] * dt * (0.4 + 2.6 * state.value)
        size = max(2.0, steps * seg['width'] / 100.0)
        k = np.arange(steps, dtype=np.float64)
        p = state.pos
        wave = (
            np.sin(2 * np.pi * (k - p) / size)
            + 0.8 * np.sin(2 * np.pi * (0.73 * k + 0.41 * p) / (1.37 * size) + 1.7)
            + 0.5 * np.sin(2 * np.pi * (k + 0.3 * p) / (2.3 * size) + 4.1)
        )
        blob = np.clip((wave / 2.3 + 1.0) / 2.0, 0.0, 1.0) ** 1.6
        hue = k / steps + p / (3.0 * steps) + 0.25 * blob
        hue = 1.0 - np.abs(np.mod(hue, 2.0) - 1.0)  # back and forth, no seam
        level = seg['floor'] + (1.0 - seg['floor']) * state.value
        return self.gradient(seg['palette'], hue) * (blob * level)[:, None]

    def _fire(self, state, seg, steps, dt):
        """
        LedFx Fire: sparks at the start heat up the segment, the heat rises
        at `speed` and cools on the way. The louder the band, the higher the
        flames reach. Colors go from the first (cool) to the last (hot).
        """
        heat = self._buffer(state, (steps,))
        level = seg['floor'] + (1.0 - seg['floor']) * state.value
        state.pos += dt
        moved = min(steps, self._advance(state, seg['speed'], dt))
        if moved:
            heat[moved:] = heat[:-moved]
            heat[:moved] = 0.0
        if steps > 2:
            heat[1:-1] = heat[1:-1] * 0.5 + (heat[:-2] + heat[2:]) * 0.25
        # Cooling, uneven along the segment so the flames form tongues
        k = np.arange(steps) / float(steps)
        swirl = np.sin(2 * np.pi * (k / 0.23 - state.pos * 1.3)) * np.sin(
            2 * np.pi * (k / 0.11 + state.pos * 0.7)
        )
        height = max(2.0, steps * (0.2 + 0.8 * level))
        fade = height / max(1.0, seg['speed']) / 1.4  # ~25% left at the tip
        heat *= np.exp(-dt / fade * (1.0 + 0.7 * swirl))
        # Bursts of sparks at the base, more and hotter when it's loud
        if self.rng.random() < 0.3 + 0.5 * level:
            base = max(1, round(steps * 0.06))
            burst = self.rng.uniform(0.6, 1.0) * (0.55 + 0.45 * level)
            spread = burst * self.rng.uniform(0.85, 1.0, base)
            np.maximum(heat[:base], spread, out=heat[:base])
        np.clip(heat, 0.0, 1.0, out=heat)
        bright = np.clip(heat * 1.8, 0.0, 1.0)
        return self.gradient(seg['palette'], heat) * bright[:, None]

    def _motion(self, mode, state, seg, steps, dt, hit, target):
        if mode == 'scroll':
            return self._scroll(state, seg, steps, dt)
        if mode == 'drops':
            return self._drops(state, seg, steps, dt, hit, target)
        if mode == 'scan':
            return self._scan(state, seg, steps, dt)
        if mode == 'wipe':
            return self._wipe(state, seg, steps, dt, hit)
        if mode == 'flow':
            return self._flow(state, seg, steps, dt)
        return self._fire(state, seg, steps, dt)

    def render(
        self,
        config: dict[str, Any],
        spectrum: Spectrum,
        dt: float,
        beat: Any = None,
        colors: Sequence[str] = (),
        paused: bool = False,
    ) -> tuple[bytes, dict[str, tuple[float, float, float]]]:
        """
        Returns the led colors as bytes and
        {segment id: (level dB, value, recent average dB)}, unrounded (see
        shown_levels). Hits segments react to the short window but report the
        level of the long one, which is what the UI's spectrum shows.
        `beat`: a beat_tracker.BeatTracker pushed with the same audio (beat
        triggers stay dark without one). `colors`: the album art's colors,
        used instead of the palettes. `paused`: the player is paused.
        """
        base, parts, full_plan, fast_plan = self._prepare(config, tuple(colors))
        out = base.copy()
        # Every segment's band at once: the long window for all of them (what
        # the UI shows, and what level triggers use), the short one for hits
        full = self.analyzer.planned_levels(spectrum, full_plan).tolist() if full_plan else []
        fast = self.analyzer.planned_levels(spectrum, fast_plan).tolist() if fast_plan else []
        follow = smoothing(AVERAGE_MS, dt)
        states = self.states
        levels = {}

        for part in parts:
            seg = part.seg
            state = states.get(part.id)
            if state is None:
                state = states[part.id] = SegmentState()
            shown = full[part.index]
            raw = fast[part.fast_index] if part.hits else shown
            if state.average is None:
                state.average = raw
            state.average += (raw - state.average) * follow
            level = raw - state.average if part.hits else raw
            if part.beats:
                target = self._beat_target(state, seg, raw, part.beats, beat, dt)
            else:
                target = self.shape_one(level, seg)
            ms = seg['attack'] if target > state.value else seg['release']
            state.value += (target - state.value) * smoothing(ms, dt)
            self._update_peak(state, dt)
            hit = self._hit(state, target, dt)
            shown_average = state.average
            if part.hits:
                if state.shown_average is None:
                    state.shown_average = shown
                state.shown_average += (shown - state.shown_average) * follow
                shown_average = state.shown_average
            levels[part.id] = (shown, state.value, shown_average)

            if not part.visible:
                continue
            leds = out[part.start : part.stop]
            mode = seg['mode']
            if mode in MOTION_MODES:
                rgb = self._motion(mode, state, seg, part.steps, dt, hit, target)[part.pos]
                np.maximum(leds, rgb, out=leds)
                continue

            floor = seg['floor']
            if mode == 'pulse':
                np.maximum(leds, part.colors * (floor + (1.0 - floor) * state.value), out=leds)
                continue
            if mode == 'shift':
                color = self.color_at(seg['palette'], state.value)
                np.maximum(leds, color * (floor + (1.0 - floor) * state.value), out=leds)
                continue
            pos, steps = part.pos, part.steps
            if mode == 'spectrum':
                bright = self._spectrum(state, seg, spectrum, part, dt)[pos]
            else:
                bright = np.clip(state.value * steps - pos, 0.0, 1.0)
                if seg['peak'] and state.peak > 0.01:
                    bright[pos == min(steps - 1, int(state.peak * steps))] = 1.0
            bright = floor + (1.0 - floor) * bright
            np.maximum(leds, part.colors * bright[:, None], out=leds)

        if len(states) > len(levels):
            for seg_id in [s for s in states if s not in levels]:
                del states[seg_id]

        out = self.ambient.blend(out, config, spectrum, dt, paused)
        if config['master'] != 1.0:
            out *= config['master']
        # np.clip's own overhead is twice that of these two
        np.maximum(out, 0.0, out=out)
        np.minimum(out, 255.0, out=out)
        return out.astype(np.uint8).tobytes(), levels


def cover_palette(seg, colors):
    """
    A segment's palette in the album art's colors: as many colors as it had,
    starting from the strongest for lows, the second for mids and the third
    for highs (by the middle of its band, like the web UI's kinds).
    """
    center = math.sqrt(seg['freq_lo'] * seg['freq_hi'])
    first = 0 if center < 250.0 else 1 if center < 4000.0 else 2
    return [colors[(first + i) % len(colors)] for i in range(len(seg['palette']))]


class HyperionSender:
    """
    Sends the latest frame to hyperion at the configured rate, when it has
    changed or KEEPALIVE seconds after it was last sent.
    """

    def __init__(self, engine):
        self.engine = engine
        self.status = 'starting'
        self.running = False
        self.client = None
        self.target = None
        self.thread = None

    def start(self):
        self.running = True
        self.thread = threading.Thread(target=self._loop)
        self.thread.daemon = True
        self.thread.start()

    def stop(self):
        self.running = False
        if self.thread is not None:
            self.thread.join(3)

    def _close(self):
        if self.client is not None:
            self.client.disconnect()
            self.client = None

    def _loop(self):
        retry_at = 0.0
        sent, sent_at = None, 0.0
        while self.running:
            settings = self.engine.store.config['hyperion']
            if not settings['enabled']:
                self._close()
                self.status = 'off'
                time.sleep(0.2)
                continue

            target = (settings['host'], settings['port'], settings['priority'])
            if self.client is not None and target != self.target:
                self._close()

            if self.client is None:
                if time.time() < retry_at:
                    time.sleep(0.2)
                    continue
                client = JsonClient(
                    settings['host'],
                    settings['port'],
                    timeout=3,
                    priority=settings['priority'],
                    duration=HYPERION_DURATION,
                )
                client.connect()
                if not client.connected:
                    self.status = 'disconnected'
                    retry_at = time.time() + RECONNECT_DELAY
                    continue
                self.client = client
                self.target = target
                self.status = 'connected'
                sent = None

            frame = self.engine.frame
            now = time.perf_counter()
            if frame and (frame != sent or now - sent_at >= KEEPALIVE):
                self.client.send_led_data(bytearray(frame))
                if not self.client.connected:
                    self.client = None
                    self.status = 'disconnected'
                    retry_at = time.time() + RECONNECT_DELAY
                    continue
                sent, sent_at = frame, now
            time.sleep(1.0 / settings['fps'])
        self._close()


class StudioEngine:
    """
    Analyses the audio from the input thread (audio_input.AudioInput) and
    keeps the latest spectrum, led frame and segment levels for the web UI
    and the hyperion sender.
    """

    def __init__(self, store):
        self.store = store
        self.analyzer = Analyzer()
        self.renderer = Renderer(self.analyzer)
        self.level = AutoLevel()
        self.beat = beat_tracker.BeatTracker(self.analyzer.freqs, SAMPLERATE)
        self._beat_configs = (None, False)  # (config, whether a segment uses the beat)
        self._watched = 0.0  # when a page last streamed
        self.media = now_playing.NowPlaying(store)
        self._track = 0  # the song the beat tracker has been following
        self.input = audio_input.AudioInput(store, self._on_audio)
        self._source = None  # the audio source auto level has been following
        self.sender = HyperionSender(self)
        self.lock = threading.Lock()
        # The capture thread and the idle thread both render
        self._render_lock = threading.Lock()
        self._last_render = time.perf_counter()
        self.spectrum = Spectrum.silent()
        self.sequence = 0  # counts analysed packets
        self._display = (-1, [], {})  # what the UI shows of one packet: (sequence, bands, levels)
        self._display_plan = self.analyzer.band_plan(self.analyzer.display_edges)
        self.frame: bytes = b''
        self.levels = {}
        self.fps = 0.0
        self.running = False
        self._frames = 0
        self._fps_time = time.perf_counter()

    def start(self):
        self.running = True
        self.media.start()
        self.input.start()
        idle = threading.Thread(target=self._idle_loop)
        idle.daemon = True
        idle.start()
        self.sender.start()

    def stop(self):
        self.running = False
        self.input.stop()
        self.media.stop()
        self.sender.stop()

    def _idle_loop(self):
        """
        Renders silence while no audio packets arrive, so time keeps moving
        for the ambient light, the release of the segments and the moving
        modes. Packets stop when nothing plays on some devices, or when the
        capture fails.
        """
        while self.running:
            time.sleep(1.0 / IDLE_FPS)
            gap = time.perf_counter() - self._last_render
            if gap >= IDLE_AFTER:
                with self._render_lock:
                    dt = min(MAX_DT, gap)
                    silence = Spectrum.silent()
                    self._track_beat(silence, int(dt * SAMPLERATE))
                    self._render(silence, dt, packet=False)

    def _beat_wanted(self, config):
        """Whether to track the beat: a segment uses it, or a page shows the tempo."""
        if self._beat_configs[0] is not config:
            used = any(s['trigger'] == 'beat' and s['enabled'] for s in config['segments'])
            self._beat_configs = (config, used)
        return self._beat_configs[1] or time.perf_counter() - self._watched < BEAT_WATCH

    def _track_beat(self, spectrum, frames):
        # Called with _render_lock held, before rendering the same audio
        config = self.store.config
        if not self._beat_wanted(config):
            self.beat.skip()
            return
        if self.media.track != self._track:
            # A new song: find its tempo afresh
            self._track = self.media.track
            self.beat.reset()
        self.beat.lead = beat_tracker.ONSET_DELAY + config['beat']['offset'] / 1000.0
        self.beat.push(spectrum, frames)

    def _render(self, spectrum, dt, packet):
        # Called with _render_lock held. fps counts audio packets only.
        config = self.store.config
        media = self.media
        frame, levels = self.renderer.render(
            config,
            spectrum,
            dt,
            beat=self.beat,
            colors=media.colors if config['now_playing']['cover_colors'] else (),
            paused=media.paused,
        )
        now = self._last_render = time.perf_counter()
        with self.lock:
            self.spectrum = spectrum
            self.frame = frame
            self.levels = levels
            self.sequence += 1
        self._frames += int(packet)
        if now - self._fps_time >= 1.0:
            self.fps = self._frames / (now - self._fps_time)
            self._frames = 0
            self._fps_time = now

    def _on_audio(
        self, frames: npt.NDArray[np.float32], peaks: npt.NDArray[np.float32], fresh: bool
    ) -> None:
        """
        New audio from the input thread. `fresh` after a gap: the analysis
        window starts empty instead of mixing in what played before it.
        """
        audio = self.store.config['audio']
        with self._render_lock:
            source = (audio['device'], audio['app'], audio['app_mode'])
            if source != self._source:
                self._source = source
                self.level.reset()
            if fresh:
                self.analyzer.clear()
            self.analyzer.offset = audio['gain'] + (self.level.gain if audio['auto_level'] else 0.0)
            spectrum = self.analyzer.push(frames, peaks)
            dt = min(MAX_DT, len(frames) / float(SAMPLERATE))
            self.level.update(spectrum.loudest, dt)
            self._track_beat(spectrum, len(frames))
            self._render(spectrum, dt, packet=True)

    def snapshot(self):
        """
        Latest state for the web UI.
        """
        self._watched = time.perf_counter()
        with self.lock:
            spectrum, frame, levels, sequence = (
                self.spectrum,
                self.frame,
                self.levels,
                self.sequence,
            )
        # Every open page streams the same display bands and levels: work them
        # out once per frame, and only for frames that are streamed
        display = self._display
        if display[0] != sequence:
            bands = self.analyzer.planned_levels(spectrum, self._display_plan)
            display = self._display = (
                sequence,
                np.round(bands).astype(int).tolist(),
                shown_levels(levels),
            )
        return {
            'led_count': self.store.config['led_count'],
            'revision': self.store.revision,
            'server_id': self.store.server_id,
            'spectrum': display[1],
            'leds': frame.hex(),
            'levels': display[2],
            'status': {
                'audio': {
                    'ok': self.input.state == 'ok',
                    'device': self.input.label,
                    'message': self.input.message,
                },
                'input': dict(
                    self.input.status(),
                    gain=self.store.config['audio']['gain'],
                    auto=round(self.level.gain, 1),
                ),
                'hyperion': self.sender.status,
                'fps': round(self.fps),
                # 0 reactive ... 1 ambient light, and seconds of silence so far
                'ambient': round(self.renderer.ambient.amount, 3),
                'quiet': round(self.renderer.ambient.quiet, 1),
                'beat': self.beat_status(),
                'media': self.media.status(),
            },
        }

    def beat_status(self):
        beat = self.beat
        return {
            'bpm': round(beat.bpm, 1),
            'confidence': round(beat.confidence, 2),
            'locked': beat.locked,
            # share of the current beat gone by, and which beat of the bar it is (0 first)
            'phase': round(beat.phase, 2),
            'bar_beat': beat.bar_beat,
        }
