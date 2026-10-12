"""
Engine of the FOLO studio: captures what is playing, analyses the spectrum,
renders the configured segments to led colors and sends them to hyperion.
"""

import math
import threading
import time

import numpy as np

from app import audio_input
from app.json_client import JsonClient

SAMPLERATE = 48000
FFT_SIZE = 4096             # spectrum and level triggers: 11.7 Hz bins, 85 ms window
FAST_FFT_SIZE = 2048        # hits triggers: half the window, so hits light 20-30 ms sooner
MAX_DT = 0.1                # longest step the renderer takes after a gap in the audio
SILENCE_DB = -120.0
# A channel takes part in the analysis once it gets louder than this. The
# unused channels of the 5.1 output aren't digital silence, they hiss at -190 dB.
ACTIVE_LEVEL = 10.0 ** (-100.0 / 20.0)
# The dB scale the presets were tuned on. The engine used to average all six
# channels of the 5.1 output, four of them silent, which reads 9.5 dB below
# the level of the stereo pair. It now averages the power of the channels
# that carry sound and keeps that scale, whatever the channel layout.
CALIBRATION_DB = -20.0 * math.log10(3.0)

DISPLAY_BANDS = 192
DISPLAY_MIN_HZ = 20.0
DISPLAY_MAX_HZ = 20000.0

AVERAGE_MS = 1000           # "recent average" that hits are measured against
PEAK_HOLD = 0.5             # seconds the peak marker of fill mode holds
PEAK_FALL = 0.8             # peak marker fall speed in segment lengths/second

# Moving modes ported from LedFx effects. They draw colors per step
# themselves; drops and wipe fire on hits (the level jumping past HIT_ON
# after falling below HIT_OFF).
MOTION_MODES = ('scroll', 'drops', 'scan', 'wipe', 'flow', 'fire')
HIT_ON = 0.5
HIT_OFF = 0.25
HIT_GAP = 0.06              # shortest time between two hits of a segment
MAX_DROPS = 32

# Ambient light (config 'reactive' and 'ambient'): what the leds show when
# audio reactiveness is off, or in auto mode after a stretch of silence
QUIET_DB = -70.0            # audio counts as silent while its loudest bin is below this
AMBIENT_FADE_IN = 1.5       # seconds to fade into the ambient light
AMBIENT_FADE_OUT = 0.15     # seconds back to the reactive look, so the first beat shows
BREATHE_PERIOD = 8.0        # seconds per breath of the breathe style
BREATHE_LOW = 0.3           # breathe style: dimmest point, as a share of the color
# Without audio packets (nothing playing on some devices, or no device) the
# engine still renders, so the ambient light fades in and keeps going
IDLE_AFTER = 0.1            # seconds without a packet before idle rendering starts
IDLE_FPS = 30

# Auto level (config audio.auto_level): follows how loud the music's loud
# moments are and adds gain so they sit where the presets were tuned (the hot
# output of the original setup). Silence holds the gain where it was.
AUTO_TARGET_DB = -15.0      # loudest bin of loud moments, after the gain
AUTO_MIN_DB = -12.0
AUTO_MAX_DB = 30.0
AUTO_RISE_MS = 300          # how fast it follows louder music
AUTO_FALL_MS = 8000         # and quieter music

# Hyperion drops our colors after this many ms without updates, so the leds
# are released even if the studio is killed
HYPERION_DURATION = 2000
RECONNECT_DELAY = 5.0


def audio_sources():
    """
    The default output, the devices and the apps playing audio (see
    audio_input.sources).
    """
    return audio_input.sources()


def hex_to_rgb(color):
    return (int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16))


def smoothing(ms, dt):
    """
    Share of the distance to the target covered in dt seconds.
    """
    if ms <= 0:
        return 1.0
    return 1.0 - math.exp(-dt * 1000.0 / ms)


class Spectrum(object):
    """
    dB levels of the latest samples: `full` from the FFT_SIZE window (the
    spectrum the UI shows, level triggers), `fast` from the FAST_FFT_SIZE one
    (hits triggers, worked out the first time something asks). Built from
    arrays with one SILENCE_DB bin of padding at the top, which band_levels()
    needs.
    """
    __slots__ = ('full', 'padded', '_fast', 'offset')

    def __init__(self, full_padded, fast, offset=0.0):
        self.full = full_padded[:-1]
        self.padded = {False: full_padded}
        self._fast = fast           # padded array, or a function returning it
        self.offset = offset        # input gain included in the levels

    @property
    def loudest(self):
        """The loudest bin without the input gain: how loud the source really is."""
        return float(self.full.max()) - self.offset

    def padded_for(self, fast):
        if fast and fast not in self.padded:
            self.padded[True] = self._fast() if callable(self._fast) else self._fast
        return self.padded[fast]

    @property
    def fast(self):
        return self.padded_for(True)[:-1]

    @classmethod
    def silent(cls):
        return cls(np.full(FFT_SIZE // 2 + 2, SILENCE_DB), np.full(FAST_FFT_SIZE // 2 + 2, SILENCE_DB))


class Analyzer(object):
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
        self.samples = np.zeros((0, FFT_SIZE))      # channels x latest samples
        self.active = np.zeros(0, dtype=bool)
        self.offset = 0.0                           # input gain in dB, added to every level
        self._windows = {}
        for size in (FFT_SIZE, FAST_FFT_SIZE):
            window = np.hanning(size)
            self._windows[size] = (window, (2.0 / window.sum()) ** 2)
        self._plans = {}

    def clear(self):
        """
        Forgets the samples, after a gap in the audio: they no longer belong
        to what's playing.
        """
        self.samples[:] = 0.0

    def push(self, frames, peaks=None):
        """
        Adds captured frames (samples x channels, or mono) and returns the new
        Spectrum. `peaks`: each channel's peak in the frames, if the caller
        has them already.
        """
        frames = np.asarray(frames, dtype=np.float64)
        if frames.ndim == 1:
            frames = frames[:, None]
        frames = frames.T
        if len(frames) != len(self.samples):
            self.samples = np.zeros((len(frames), FFT_SIZE))
            self.active = np.zeros(len(frames), dtype=bool)
        count = frames.shape[1]
        if count >= FFT_SIZE:
            self.samples = np.ascontiguousarray(frames[:, -FFT_SIZE:])
        elif count:
            self.samples[:, :-count] = self.samples[:, count:]
            self.samples[:, -count:] = frames
        # A channel counts from the first time it carries sound
        if count:
            if peaks is None or len(peaks) != len(frames):
                peaks = np.abs(frames).max(axis=1)
            self.active |= np.asarray(peaks) > ACTIVE_LEVEL
        if not self.active.any():
            return Spectrum.silent()
        playing = self.samples[self.active]
        offset = self.offset
        return Spectrum(self._db(playing, FFT_SIZE, offset), lambda: self._db(playing, FAST_FFT_SIZE, offset), offset)

    def _db(self, playing, size, offset):
        window, norm = self._windows[size]
        bins = np.fft.rfft(playing[:, -size:] * window, axis=1)
        power = (bins.real ** 2 + bins.imag ** 2).mean(axis=0) * norm
        db = np.full(len(power) + 1, SILENCE_DB)
        np.maximum(10.0 * np.log10(np.maximum(power, 1e-30)) + (CALIBRATION_DB + offset), SILENCE_DB, out=db[:-1])
        return db

    def band_levels(self, spectrum, edges, fast=False):
        """
        Loudest bin between each pair of edges (Hz), from the short window if
        `fast`. Bands narrower than the FFT resolution are interpolated.
        """
        padded = spectrum.padded_for(fast)
        db = padded[:-1]
        freqs = self.fast_freqs if fast else self.freqs
        edges = np.asarray(edges, dtype=np.float64)
        key = (fast, edges.tobytes())
        plan = self._plans.get(key)
        if plan is None:
            if len(self._plans) > 512:
                self._plans.clear()
            idx = np.searchsorted(freqs, edges)
            empty = idx[1:] <= idx[:-1]
            centers = np.sqrt(edges[:-1] * edges[1:])[empty]
            plan = self._plans[key] = (idx, empty, centers)
        idx, empty, centers = plan
        # reduceat takes the max of db[idx[i]:idx[i + 1]]; the padding keeps
        # an index at the top edge valid
        levels = np.maximum.reduceat(padded, idx)[:-1]
        if centers.size:
            levels[empty] = np.interp(centers, freqs, db)
        return levels

    def band_level(self, spectrum, freq_lo, freq_hi, fast=False):
        return float(self.band_levels(spectrum, (freq_lo, freq_hi), fast)[0])


class SegmentState(object):
    def __init__(self):
        self.value = 0.0
        self.peak = 0.0
        self.peak_age = 0.0
        self.bands = None
        self.average = None
        self.shown_average = None   # hits: average of the level the UI's spectrum shows
        self.band_average = None
        # moving modes
        self.armed = True
        self.since_hit = 1.0
        self.buffer = None          # scroll trail colors or fire heat, per step
        self.carry = 0.0            # fraction of a step not moved yet
        self.drops = []             # [center step, age s, rgb, strength]
        self.pos = 0.0              # scan eye start, wipe progress or flow phase
        self.heading = 1.0
        self.color_index = 0


class AutoLevel(object):
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

    def update(self, loudest, dt):
        if loudest < QUIET_DB:
            return
        if self.envelope is None:
            self.envelope = loudest
        ms = AUTO_RISE_MS if loudest > self.envelope else AUTO_FALL_MS
        self.envelope += (loudest - self.envelope) * smoothing(ms, dt)
        self.gain = min(AUTO_MAX_DB, max(AUTO_MIN_DB, AUTO_TARGET_DB - self.envelope))


class Ambient(object):
    """
    Crossfades between the reactive frame and the ambient light. `amount` is
    0 for the reactive look and 1 for the ambient light; `quiet` counts the
    seconds the audio has been silent.
    """
    def __init__(self):
        self.amount = 0.0
        self.quiet = 0.0
        self.phase = 0.0

    def wanted(self, config):
        mode = config['reactive']
        if mode == 'off':
            return 1.0
        if mode == 'auto' and self.quiet >= config['ambient']['idle_after']:
            return 1.0
        return 0.0

    def update(self, config, spectrum, dt):
        self.quiet = 0.0 if spectrum.loudest >= QUIET_DB else self.quiet + dt
        self.phase = (self.phase + dt / BREATHE_PERIOD) % 1.0
        want = self.wanted(config)
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

    def blend(self, out, config, spectrum, dt):
        """
        Mixes the ambient light into `out` (leds x rgb, before `master`).
        """
        self.update(config, spectrum, dt)
        if self.amount <= 0.0:
            return out
        a = self.amount * self.amount * (3.0 - 2.0 * self.amount)     # eased
        return out * (1.0 - a) + self.color(config) * a


class Renderer(object):
    """
    Turns the spectrum into led colors according to the segments, and blends
    in the ambient light.
    """
    def __init__(self, analyzer):
        self.analyzer = analyzer
        self.ambient = Ambient()
        self.states = {}
        self.rng = np.random.default_rng()
        self._layouts = {}
        self._gradients = {}

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
        colors = np.array([hex_to_rgb(c) for c in palette], dtype=np.float64)
        if len(colors) == 1:
            return np.repeat(colors, len(t), axis=0)
        pos = np.clip(t, 0.0, 1.0) * (len(colors) - 1)
        i = np.minimum(pos.astype(int), len(colors) - 2)
        f = (pos - i)[:, None]
        return colors[i] * (1.0 - f) + colors[i + 1] * f

    def static_gradient(self, palette, size, direction, t):
        key = (tuple(palette), size, direction)
        colors = self._gradients.get(key)
        if colors is None:
            if len(self._gradients) > 256:
                self._gradients.clear()
            colors = self._gradients[key] = self.gradient(palette, t)
        return colors

    def step_gradient(self, palette, steps):
        return self.static_gradient(palette, steps, 'steps', np.linspace(0.0, 1.0, steps))

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
    def _update_peak(state, dt):
        if state.value >= state.peak:
            state.peak = state.value
            state.peak_age = 0.0
        else:
            state.peak_age += dt
            if state.peak_age > PEAK_HOLD:
                state.peak = max(state.value, state.peak - PEAK_FALL * dt)

    def _spectrum(self, state, seg, spectrum, steps, dt):
        edges = np.geomspace(seg['freq_lo'], seg['freq_hi'], steps + 1)
        levels = self.analyzer.band_levels(spectrum, edges, fast=seg['trigger'] == 'hits')
        if seg['trigger'] == 'hits':
            if state.band_average is None or len(state.band_average) != steps:
                state.band_average = levels.copy()
            state.band_average += (levels - state.band_average) * smoothing(AVERAGE_MS, dt)
            levels = levels - state.band_average
        target = self.shape(levels, seg)
        if state.bands is None or len(state.bands) != steps:
            state.bands = np.zeros(steps)
        rate = np.where(target > state.bands,
                        smoothing(seg['attack'], dt), smoothing(seg['release'], dt))
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
        fade = length / max(1.0, seg['speed']) / 3.0     # ~5% left at the end
        trail *= math.exp(-dt / fade)
        color = self.gradient(seg['palette'], np.array([state.value]))[0] * state.value
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
            color = self.gradient(seg['palette'], self.rng.random(1))[0]
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
        color = self.gradient(seg['palette'], np.array([(state.pos + length / 2.0) / steps]))[0]
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
        front = (1.0 - (1.0 - state.pos) ** 2) * steps     # eases out like LedFx
        k = np.arange(steps) + 0.5
        s = k if state.heading > 0 else steps - k
        behind = np.clip(front - s + 0.5, 0.0, 1.0)
        new = np.array(hex_to_rgb(palette[state.color_index % len(palette)]), dtype=np.float64)
        if seg['width'] >= 100:
            old = np.array(hex_to_rgb(palette[(state.color_index - 1) % len(palette)]), dtype=np.float64)
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
        wave = (np.sin(2 * np.pi * (k - p) / size)
                + 0.8 * np.sin(2 * np.pi * (0.73 * k + 0.41 * p) / (1.37 * size) + 1.7)
                + 0.5 * np.sin(2 * np.pi * (k + 0.3 * p) / (2.3 * size) + 4.1))
        blob = np.clip((wave / 2.3 + 1.0) / 2.0, 0.0, 1.0) ** 1.6
        hue = k / steps + p / (3.0 * steps) + 0.25 * blob
        hue = 1.0 - np.abs(np.mod(hue, 2.0) - 1.0)      # back and forth, no seam
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
        swirl = (np.sin(2 * np.pi * (k / 0.23 - state.pos * 1.3))
                 * np.sin(2 * np.pi * (k / 0.11 + state.pos * 0.7)))
        height = max(2.0, steps * (0.2 + 0.8 * level))
        fade = height / max(1.0, seg['speed']) / 1.4       # ~25% left at the tip
        heat *= np.exp(-dt / fade * (1.0 + 0.7 * swirl))
        # Bursts of sparks at the base, more and hotter when it's loud
        if self.rng.random() < 0.3 + 0.5 * level:
            base = max(1, int(round(steps * 0.06)))
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

    def render(self, config, spectrum, dt):
        """
        Returns the led colors as bytes and
        {segment id: [level dB, value, recent average dB]}. Hits segments
        react to the short window but report the level of the long one, which
        is what the UI's spectrum shows.
        """
        count = config['led_count']
        background = np.array(hex_to_rgb(config['background']), dtype=np.float64)
        out = np.tile(background, (count, 1))
        segments = config['segments']
        solo = any(s['solo'] and s['enabled'] for s in segments)
        levels = {}

        for seg in segments:
            state = self.states.setdefault(seg['id'], SegmentState())
            hits = seg['trigger'] == 'hits'
            raw = self.analyzer.band_level(spectrum, seg['freq_lo'], seg['freq_hi'], fast=hits)
            if state.average is None:
                state.average = raw
            state.average += (raw - state.average) * smoothing(AVERAGE_MS, dt)
            level = raw - state.average if hits else raw
            target = float(self.shape(level, seg))
            ms = seg['attack'] if target > state.value else seg['release']
            state.value += (target - state.value) * smoothing(ms, dt)
            self._update_peak(state, dt)
            hit = self._hit(state, target, dt)
            shown, shown_average = raw, state.average
            if hits:
                shown = self.analyzer.band_level(spectrum, seg['freq_lo'], seg['freq_hi'])
                if state.shown_average is None:
                    state.shown_average = shown
                state.shown_average += (shown - state.shown_average) * smoothing(AVERAGE_MS, dt)
                shown_average = state.shown_average
            levels[seg['id']] = [round(shown, 1), round(state.value, 3), round(shown_average, 1)]

            start = seg['start']
            end = min(seg['end'], count - 1)
            if not seg['enabled'] or (solo and not seg['solo']) or end < start:
                continue

            size = end - start + 1
            pos, steps = self.layout(size, seg['direction'])
            mode = seg['mode']
            if mode in MOTION_MODES:
                rgb = self._motion(mode, state, seg, steps, dt, hit, target)[pos]
                out[start:end + 1] = np.maximum(out[start:end + 1], rgb)
                continue

            t = pos / float(max(1, steps - 1))
            if mode == 'shift':
                colors = self.gradient(seg['palette'], np.array([state.value]))
                bright = np.full(size, state.value)
            else:
                colors = self.static_gradient(seg['palette'], size, seg['direction'], t)
                if mode == 'pulse':
                    bright = np.full(size, state.value)
                elif mode == 'spectrum':
                    bright = self._spectrum(state, seg, spectrum, steps, dt)[pos]
                else:
                    bright = np.clip(state.value * steps - pos, 0.0, 1.0)
                    if seg['peak'] and state.peak > 0.01:
                        bright[pos == min(steps - 1, int(state.peak * steps))] = 1.0

            bright = seg['floor'] + (1.0 - seg['floor']) * bright
            out[start:end + 1] = np.maximum(out[start:end + 1], colors * bright[:, None])

        for seg_id in list(self.states):
            if seg_id not in levels:
                del self.states[seg_id]

        out = self.ambient.blend(out, config, spectrum, dt)
        out *= config['master']
        return np.clip(out, 0, 255).astype(np.uint8).tobytes(), levels


class HyperionSender(object):
    """
    Sends the latest frame to hyperion at the configured rate.
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
                client = JsonClient(settings['host'], settings['port'], timeout=3,
                                    priority=settings['priority'],
                                    duration=HYPERION_DURATION)
                client.connect()
                if not client.connected:
                    self.status = 'disconnected'
                    retry_at = time.time() + RECONNECT_DELAY
                    continue
                self.client = client
                self.target = target
                self.status = 'connected'

            frame = self.engine.frame
            if frame:
                self.client.send_led_data(bytearray(frame))
                if not self.client.connected:
                    self.client = None
                    self.status = 'disconnected'
                    retry_at = time.time() + RECONNECT_DELAY
                    continue
            time.sleep(1.0 / settings['fps'])
        self._close()


class StudioEngine(object):
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
        self.input = audio_input.AudioInput(store, self._on_audio)
        self._source = None         # the audio source auto level has been following
        self.sender = HyperionSender(self)
        self.lock = threading.Lock()
        # The capture thread and the idle thread both render
        self._render_lock = threading.Lock()
        self._last_render = time.perf_counter()
        self.spectrum = Spectrum.silent()
        self.sequence = 0           # counts analysed packets
        self._display = (-1, [])    # display bands of one packet: (sequence, bands)
        self.frame = b''
        self.levels = {}
        self.fps = 0.0
        self.running = False
        self._frames = 0
        self._fps_time = time.perf_counter()

    def start(self):
        self.running = True
        self.input.start()
        idle = threading.Thread(target=self._idle_loop)
        idle.daemon = True
        idle.start()
        self.sender.start()

    def stop(self):
        self.running = False
        self.input.stop()
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
                    self._render(Spectrum.silent(), min(MAX_DT, gap), packet=False)

    def _render(self, spectrum, dt, packet):
        # Called with _render_lock held. fps counts audio packets only.
        frame, levels = self.renderer.render(self.store.config, spectrum, dt)
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

    def _on_audio(self, frames, peaks, fresh):
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
            self._render(spectrum, dt, packet=True)

    def snapshot(self):
        """
        Latest state for the web UI.
        """
        with self.lock:
            spectrum, frame, levels, sequence = self.spectrum, self.frame, self.levels, self.sequence
        # Every open page streams the same display bands: work them out once per frame
        display = self._display
        if display[0] != sequence:
            bands = self.analyzer.band_levels(spectrum, self.analyzer.display_edges)
            display = self._display = (sequence, np.round(bands).astype(int).tolist())
        return {
            'led_count': self.store.config['led_count'],
            'revision': self.store.revision,
            'server_id': self.store.server_id,
            'spectrum': display[1],
            'leds': frame.hex(),
            'levels': levels,
            'status': {
                'audio': {'ok': self.input.state == 'ok', 'device': self.input.label, 'message': self.input.message},
                'input': dict(self.input.status(), gain=self.store.config['audio']['gain'],
                              auto=round(self.level.gain, 1)),
                'hyperion': self.sender.status,
                'fps': int(round(self.fps)),
                # 0 reactive ... 1 ambient light, and seconds of silence so far
                'ambient': round(self.renderer.ambient.amount, 3),
                'quiet': round(self.renderer.ambient.quiet, 1),
            },
        }
