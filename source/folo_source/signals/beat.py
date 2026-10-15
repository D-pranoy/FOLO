"""
Beat tracking for the FOLO studio: finds the tempo and the beats of what is
playing from the analyzer's spectra, and keeps a beat clock running in step
with them, a little ahead of the audio so the lights land on the beat.

- Onsets: how much ONSET_BANDS log-spaced bands (their loudest bin) got
  louder over ONSET_LAG, in dB, averaged over the bands (spectral flux), one
  value per HOP.
- Tempo: the autocorrelation of the last WINDOW seconds of onsets (slow
  swells taken out, older onsets weighing less) at each candidate beat period
  and its multiples, weighted towards PRIOR_BPM and towards the tempo it has
  already found.
- Lock: beats only fire while that autocorrelation (`confidence`) has stayed
  above LOCK_ON for LOCK_AFTER, so music without a steady beat (ambient,
  speech, free piano) stays unlocked.
- Phase: the offset at which a comb of beats one period apart best lines up
  with the recent onsets.
- Clock: `pos` counts beats and only ever moves forward. It runs at the
  tempo and speeds up or slows down a little (at most MAX_CORRECTION) to make
  up phase errors over PHASE_TAU, so a beat never fires twice or gets skipped.
- Bars: of the four beats, the one with the strongest bass onsets and the
  biggest change of sound since the beat before (chords and bass notes change
  on the first beat, drums repeat) is taken as the first (`downbeat`).

Measured on synthetic songs (60-180 BPM, four-on-the-floor, rock, swung
hip-hop, tempo drift, breakdowns): it locks after about 4 s, every beat lands
within 10 ms, and it costs about 0.6% of a core.
"""

import collections
import math
from typing import TYPE_CHECKING

import numpy as np

if TYPE_CHECKING:
    from folo_source.inputs.audio.analyzer import Spectrum

HOP = 0.01  # seconds between onset values
WINDOW = 8.0  # seconds of onsets the tempo is worked out from
MIN_HISTORY = 3.0  # seconds of onsets before the first estimate
UPDATE_EVERY = 0.1  # seconds between tempo and phase estimates
RECENT = 3.0  # onsets this old count 1/e as much for the tempo
DETREND = 1.0  # seconds of onsets averaged to take out slow swells
ONSET_BANDS = 24
ONSET_LO_HZ = 40.0
ONSET_HI_HZ = 10000.0
ONSET_LAG = 0.02  # onsets compare the bands with the spectrum this much older
ONSET_RANGE_DB = 50.0  # bands quieter than this below the loudest don't count
TOP_FALL_DB = 6.0  # dB per second the loudest band's envelope falls
BASS_HZ = 150.0  # bands below this: bass onsets, accents that find the bar
TONAL_HZ = 4000.0  # bands below this: changes of sound (chords), the other accents
MIN_BPM = 60.0
MAX_BPM = 200.0
BPM_STEP = 0.5
PRIOR_BPM = 120.0  # tempos near this are preferred, by a log-Gaussian
PRIOR_OCTAVES = 1.0  # ... this wide
STAY_BONUS = 0.3  # extra weight for the tempo already found
STAY_OCTAVES = 0.03
TEMPO_FOLLOW = 0.25  # share of a small tempo change taken per estimate
TEMPO_JUMP = 0.04  # log ratio beyond which a new tempo must persist
JUMP_AFTER = 1.0  # ... this many seconds before the clock takes it
COMB_SECONDS = 4.0  # how far back the phase comb reaches
COMB_DECAY = 0.85  # weight of each older beat in the comb
PHASE_TAU = 0.8  # seconds over which phase errors are made up
MAX_CORRECTION = 0.25  # most the clock speeds up or slows down, as a share of the tempo
LOCK_ON = 0.3  # periodicity (autocorrelation at the beat) to lock
LOCK_OFF = 0.2  # ... and below which the lock is lost
LOCK_AFTER = 0.5  # seconds the periodicity must stay above LOCK_ON
CONFIDENCE_FOLLOW = 0.1  # share of each estimate taken into the confidence
QUIET_DB = -70.0  # loudest bin without gain below this counts as silence
UNLOCK_QUIET = 1.0  # seconds of silence that stop the beats
FORGET_QUIET = 3.0  # seconds of silence after which the tempo is forgotten
ACCENT_DECAY = 0.8  # per bar, for the accents of the four beats
DOWNBEAT_MARGIN = 1.15  # a new first beat must have this many more accents
# The onsets lag the audio: the analysis window has to take in part of a sound
# before its bands rise. Measured on synthetic kicks (about +5 ms after this)
# and clicks (-10 ms). Beats are fired this much earlier.
ONSET_DELAY = 0.025


class BeatTracker:
    """
    Feed it every spectrum with push(spectrum, frames). `pos` is the beat
    clock after the latest push and `before` where it was before it; a beat
    fired when floor(pos) passed an integer. `lead` (seconds) fires the beats
    that much before they are heard in the captured audio.
    """

    def __init__(self, freqs, rate=48000):
        self.rate = rate
        self.hop_frames = round(HOP * rate)
        edges = np.searchsorted(freqs, np.geomspace(ONSET_LO_HZ, ONSET_HI_HZ, ONSET_BANDS + 1))
        edges = np.unique(edges)
        self._starts = edges[:-1]
        self._stop = int(edges[-1])
        self._lag_frames = int(ONSET_LAG * rate)
        self._detrend = (
            0,
            None,
            None,
            None,
        )  # (steps, lo, hi, width) of _estimate's moving average
        self._bass = max(1, int(np.searchsorted(freqs[edges[:-1]], BASS_HZ)))
        self._tonal = max(1, int(np.searchsorted(freqs[edges[:-1]], TONAL_HZ)))
        self._steps = round(WINDOW / HOP)
        self._bpms = np.arange(MIN_BPM, MAX_BPM + BPM_STEP / 2, BPM_STEP)
        self._lags = 60.0 / self._bpms / HOP
        self._prior = np.exp(-0.5 * (np.log2(self._bpms / PRIOR_BPM) / PRIOR_OCTAVES) ** 2)
        self._recency = np.exp(-np.arange(self._steps)[::-1] * HOP / RECENT)
        self.lead = ONSET_DELAY
        self.pos = 0.0
        self.before = 0.0
        self.downbeat = 0
        self.locked = False
        self.reset()

    def reset(self):
        """Forgets the tempo, e.g. when a new song starts. The clock keeps counting."""
        self._env = np.zeros(self._steps)
        self._bass_env = np.zeros(self._steps)
        self._write = 0  # next index of the rings
        self._filled = 0  # valid onset values in the rings
        self._frames = 0  # frames pushed since the reset
        self._step = 0  # onset values written since the reset
        self._pending = 0.0
        self._pending_bass = 0.0
        self._recent = collections.deque()  # (frames, band dB) of the latest spectra
        self._top = None
        self._since_update = 0.0
        self.quiet = 0.0
        self.period = 0.5  # seconds per beat
        self._found = False  # whether period has been estimated yet
        self.confidence: float = 0.0
        self.locked = False
        self._steady = 0.0  # seconds the confidence has been above LOCK_ON
        self._error = 0.0  # beats the clock is still behind the music
        self._pending_period = None
        self._pending_for = 0.0
        self._accents = np.zeros(4)
        self._usual = [0.0, 0.0]  # running mean of each kind of accent
        self._to_accent = collections.deque()  # (beat number, onset time to look at)
        self._beat_sum = np.zeros(len(self._starts))  # band levels summed over the current beat
        self._beat_count = 0
        self._last_mean = None  # mean band levels of the beat before

    @property
    def bpm(self) -> float:
        """The tempo found, 0 before there is one."""
        return 60.0 / self.period if self._found else 0.0

    @property
    def phase(self):
        """Share of the current beat gone by, 0-1."""
        return self.pos - math.floor(self.pos)

    @property
    def bar_beat(self):
        """Which beat of the bar this is, 0 for the first."""
        return (math.floor(self.pos) - self.downbeat) % 4

    def fired(self, every=1.0):
        """
        True if the latest push passed a beat of a grid `every` beats apart
        (0.5, 1, 2 or 4), aligned to the bar.
        """
        if not self.locked:
            return False
        offset = self.downbeat if every >= 1.0 else 0.0
        return math.floor((self.pos - offset) / every) > math.floor((self.before - offset) / every)

    # ------------------------------------------------------------- input

    def skip(self):
        """
        Not tracking for now (nothing uses the beat): no beats fire, and the
        onsets start over when push() is called again.
        """
        self.before = self.pos
        if self._filled or self.locked:
            self.reset()

    def push(self, spectrum: 'Spectrum', frames: int) -> None:
        """
        Takes the spectrum of the latest `frames` frames of audio (a
        studio_engine.Spectrum, levels in dB with the input gain) and moves
        the clock forward by their duration.
        """
        dt = frames / float(self.rate)
        self.before = self.pos
        if frames <= 0:
            return
        self._frames += frames
        loudest = spectrum.loudest
        self.quiet = self.quiet + dt if loudest < QUIET_DB else 0.0
        if self.quiet >= FORGET_QUIET and self._filled:
            self.reset()
            self.quiet = FORGET_QUIET
            self.before = self.pos
        flux, bass = self._onset(spectrum, dt) if self.quiet < UNLOCK_QUIET else (0.0, 0.0)
        self._pending = max(self._pending, flux)
        self._pending_bass = max(self._pending_bass, bass)
        steps = self._frames // self.hop_frames - self._step
        for i in range(steps):
            last = i == steps - 1
            self._env[self._write] = self._pending if last else 0.0
            self._bass_env[self._write] = self._pending_bass if last else 0.0
            self._write = (self._write + 1) % self._steps
            self._filled = min(self._steps, self._filled + 1)
        if steps:
            self._step += steps
            self._pending = self._pending_bass = 0.0

        self._since_update += dt
        if self._since_update >= UPDATE_EVERY:
            self._estimate(self._since_update)
            self._since_update = 0.0
        if self.quiet >= UNLOCK_QUIET:
            self.locked = False
        self._advance(dt)
        self._count_accents()

    def _onset(self, spectrum, dt):
        """Spectral flux of the bands since ONSET_LAG ago, all bands and the bass."""
        # The loudest bin of each band (as good as their mean power here, and
        # cheaper: this runs on every packet)
        db = np.maximum.reduceat(spectrum.full[: self._stop], self._starts)
        top = float(np.maximum.reduce(db))
        if self._top is None or top > self._top:
            self._top = top
        else:
            self._top = max(top, self._top - TOP_FALL_DB * dt)
        np.maximum(db, self._top - ONSET_RANGE_DB, out=db)
        self._beat_sum += db
        self._beat_count += 1

        recent = self._recent
        recent.append((self._frames, db))
        while len(recent) > 1 and recent[1][0] <= self._frames - self._lag_frames:
            recent.popleft()
        rise = np.subtract(db, recent[0][1])
        np.maximum(rise, 0.0, out=rise)
        return float(np.add.reduce(rise)) / len(rise), float(
            np.add.reduce(rise[: self._bass])
        ) / self._bass

    # ------------------------------------------------------------- estimates

    def _history(self, ring):
        """The valid part of a ring, oldest first."""
        ordered = np.concatenate((ring[self._write :], ring[: self._write]))
        return ordered[len(ordered) - self._filled :]

    def _estimate(self, elapsed):
        if self._filled * HOP < MIN_HISTORY:
            self._follow_confidence(0.0)
            return
        env = self._history(self._env)
        smooth = np.convolve(env, (0.25, 0.5, 0.25), mode='same')

        # Tempo: normalised autocorrelation at every candidate period and its
        # double and quadruple, which a steady beat also repeats at. Slow
        # swells are taken out first (they correlate at every lag), and older
        # onsets count less, so a new tempo shows within a few seconds.
        steps = len(smooth)
        if self._detrend[0] != steps:
            half = round(DETREND / HOP) // 2
            lo = np.clip(np.arange(steps) - half, 0, steps)
            hi = np.clip(np.arange(steps) + half + 1, 0, steps)
            self._detrend = (steps, lo, hi, (hi - lo).astype(np.float64))
        _, lo, hi, width = self._detrend
        trend = np.cumsum(np.concatenate(([0.0], smooth)))
        x = smooth - (trend[hi] - trend[lo]) / width
        x *= self._recency[-steps:]
        energy = float(np.dot(x, x))
        if energy <= 1e-12:
            self._follow_confidence(0.0)
            return
        size = 1 << math.ceil(math.log2(2 * len(x)))
        spec = np.fft.rfft(x, size)
        acf = np.fft.irfft(spec.real**2 + spec.imag**2, size)[: len(x)] / energy
        lag_axis = np.arange(len(acf))
        r = np.interp(self._lags, lag_axis, acf, right=0.0)
        score = (
            r
            + 0.5 * np.interp(2 * self._lags, lag_axis, acf, right=0.0)
            + 0.25 * np.interp(4 * self._lags, lag_axis, acf, right=0.0)
        )
        score = np.maximum(score, 0.0) * self._prior
        if self.locked:
            near = np.log2(self._bpms * self.period / 60.0)
            score *= 1.0 + STAY_BONUS * np.exp(-0.5 * (near / STAY_OCTAVES) ** 2)
        best = int(np.argmax(score))
        bpm = self._bpms[best]
        if 0 < best < len(score) - 1:
            a, b, c = score[best - 1], score[best], score[best + 1]
            bend = a - 2 * b + c
            if bend < 0:
                bpm += 0.5 * (a - c) / bend * BPM_STEP
        self._follow_tempo(60.0 / bpm, elapsed)
        self._found = True
        self._follow_confidence(float(max(0.0, r[best])))
        self._steady = self._steady + elapsed if self.confidence >= LOCK_ON else 0.0

        # Phase: where the comb of beats lines up best with the onsets
        period = self.period / HOP
        beats = int(min(COMB_SECONDS / self.period, (len(smooth) - 1) / period)) + 1
        offsets = np.arange(math.ceil(period))
        taps = (
            (len(smooth) - 1 - offsets[:, None] - np.arange(beats)[None, :] * period)
            .round()
            .astype(np.intp)
        )
        taps = np.clip(taps, 0, len(smooth) - 1)
        comb = (smooth[taps] * (COMB_DECAY ** np.arange(beats))[None, :]).sum(axis=1)
        k = int(np.argmax(comb))
        shift = float(k)
        n = len(comb)
        a, b, c = comb[(k - 1) % n], comb[k], comb[(k + 1) % n]
        bend = a - 2 * b + c
        if bend < 0:
            shift += 0.5 * (a - c) / bend
        # The newest onset is at the end of the pushed audio; the last beat
        # was `shift` steps before it, and its sound ONSET_DELAY before that
        target = (shift * HOP + self.lead) / self.period
        error = target - math.floor(target) - self.phase
        error -= math.floor(error + 0.5)
        if not self.locked and self._steady >= LOCK_AFTER:
            # Locking on: set the clock straight away, forward only
            pos = math.floor(self.pos) + self.phase + error
            self.pos = self.before = pos if pos >= self.pos else pos + 1.0
            self._error = 0.0
            self.locked = True
        else:
            self._error = error
            if self.locked and self.confidence < LOCK_OFF:
                self.locked = False

    def _follow_tempo(self, period, elapsed):
        change = math.log(period / self.period)
        if not self.locked or abs(change) < TEMPO_JUMP:
            self.period *= math.exp(change * (1.0 if not self.locked else TEMPO_FOLLOW))
            self._pending_period = None
            return
        # A different tempo has to persist before the clock takes it
        if (
            self._pending_period is None
            or abs(math.log(period / self._pending_period)) >= TEMPO_JUMP
        ):
            self._pending_period = period
            self._pending_for = 0.0
            return
        self._pending_for += elapsed
        self._pending_period = period
        if self._pending_for >= JUMP_AFTER:
            self.period = period
            self._pending_period = None
            self.locked = False  # locks on again at the new phase

    def _follow_confidence(self, value):
        self.confidence += (value - self.confidence) * CONFIDENCE_FOLLOW

    # ------------------------------------------------------------- clock

    def _advance(self, dt):
        rate = 1.0 / self.period
        limit = MAX_CORRECTION * rate
        correction = min(limit, max(-limit, self._error / PHASE_TAU))
        self.pos += (rate + correction) * dt
        self._error -= correction * dt
        if math.floor(self.pos) > math.floor(self.before):
            self._next_beat(math.floor(self.pos))

    def _next_beat(self, beat):
        """The clock reached `beat`: the accents of the beats before it can be counted."""
        # How much the sound changed from the beat before (chords and bass
        # notes change on the first beat of a bar, the drums repeat)
        if self._beat_count:
            mean = self._beat_sum / self._beat_count
            if self._last_mean is not None and self.locked:
                self._accent(
                    beat - 1, 1, float(np.abs(mean - self._last_mean)[: self._tonal].mean())
                )
            self._last_mean = mean
        self._beat_sum[:] = 0.0
        self._beat_count = 0
        if self.locked:
            # Its bass onsets show `lead` after now
            self._to_accent.append((beat, self._frames / float(self.rate) + self.lead))

    def _count_accents(self):
        now = self._frames / float(self.rate)
        while self._to_accent and self._to_accent[0][1] + 0.05 <= now:
            beat, at = self._to_accent.popleft()
            # Bass onsets within 40 ms of the beat
            first = round((now - at - 0.04) / HOP)
            last = round((now - at + 0.04) / HOP)
            history = self._history(self._bass_env)
            if first <= 0 or not len(history):
                continue
            window = history[max(0, len(history) - 1 - last) : len(history) - first]
            self._accent(beat, 0, float(window.max()) if len(window) else 0.0)

    def _accent(self, beat, kind, value):
        """
        Adds an accent (kind 0: bass onset, 1: change of sound) to its beat
        of four, relative to the usual size of that kind of accent, and picks
        the beat with the most as the first of the bar.
        """
        self._accents *= ACCENT_DECAY**0.125  # two accents per beat
        usual = self._usual[kind]
        self._usual[kind] = usual + (value - usual) * 0.05 if usual else value
        if usual <= 1e-9:
            return
        self._accents[beat % 4] += min(4.0, value / usual)
        best = int(np.argmax(self._accents))
        if self._accents[best] > self._accents[self.downbeat] * DOWNBEAT_MARGIN:
            self.downbeat = best
