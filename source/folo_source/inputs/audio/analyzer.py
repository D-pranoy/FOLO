"""
Audio analysis: Hann-windowed spectra of the latest samples (Analyzer,
Spectrum) and auto level (AutoLevel).
"""

import collections
import math

import numpy as np
import numpy.typing as npt

SAMPLERATE = 48000
FFT_SIZE = 4096  # spectrum and level triggers: 11.7 Hz bins, 85 ms window
FAST_FFT_SIZE = 2048  # hits triggers: half the window, so hits light 20-30 ms sooner
# Samples kept per channel. New audio is appended, and only when the buffer is
# full do the latest FFT_SIZE samples move back to the front.
HISTORY = 2 * FFT_SIZE
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

# Audio counts as silent while its loudest bin is below this
QUIET_DB = -70.0

# Auto level (config audio.auto_level): follows how loud the music's loud
# moments are and adds gain so they sit where the presets were tuned (the hot
# output of the original setup). Silence holds the gain where it was.
AUTO_TARGET_DB = -15.0  # loudest bin of loud moments, after the gain
AUTO_MIN_DB = -12.0
AUTO_MAX_DB = 30.0
AUTO_RISE_MS = 300  # how fast it follows louder music
AUTO_FALL_MS = 8000  # and quieter music


def channel_peaks(frames: npt.NDArray[np.floating]) -> npt.NDArray[np.floating]:
    """
    Each channel's peak in frames x channels. Transposed first: numpy is three
    times slower reducing over the short channel axis.
    """
    return np.maximum.reduce(np.abs(np.ascontiguousarray(frames.T)), axis=1)


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
                peaks = channel_peaks(frames)
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
