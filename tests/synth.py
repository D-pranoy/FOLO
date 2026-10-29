"""
Deterministic synthetic audio for the golden tests (RB-01 step 6).

Everything is made in float64, quantized to 16 bits and handed over as the
float32 frames WASAPI delivers. The quantization absorbs the last-bit
differences of sin, exp and FFTs between platforms, so the same code gives
the same samples everywhere. Noise comes from Generator.random(), whose
stream numpy keeps stable.
"""

from __future__ import annotations

import math
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field

import numpy as np
import numpy.typing as npt

RATE = 48000
PACKET = 480  # frames per packet, as the studio's capture delivers them

Floats = npt.NDArray[np.float64]
Capture = npt.NDArray[np.float32]


def capture(stereo: Floats) -> Capture:
    """Quantizes frames x 2 in -1..1 to 16 bits and returns them as float32."""
    q = np.clip(np.round(stereo * 32768.0), -32768.0, 32767.0)
    return (q / 32768.0).astype(np.float32)


def pcm16(frames: Capture) -> bytes:
    """The 16-bit samples behind captured frames, to fingerprint a signal."""
    return np.round(frames.astype(np.float64) * 32768.0).astype('<i2').tobytes()


def packets(frames: Capture) -> Iterator[Capture]:
    """480-frame packets; a shorter last one is dropped."""
    for start in range(0, len(frames) - PACKET + 1, PACKET):
        yield frames[start : start + PACKET]


def _time(seconds: float) -> Floats:
    return np.arange(round(seconds * RATE), dtype=np.float64) / RATE


def _mono(x: Floats) -> Floats:
    return np.column_stack((x, x))


def _noise(rng: np.random.Generator, n: int) -> Floats:
    return rng.random(n) * 2.0 - 1.0


def _pink(rng: np.random.Generator, n: int) -> Floats:
    """Pink noise (1/f power) with an RMS of 1."""
    spectrum = np.fft.rfft(_noise(rng, n))
    f = np.fft.rfftfreq(n, 1.0 / RATE)
    scale = np.zeros_like(f)
    scale[f >= 10.0] = 1.0 / np.sqrt(f[f >= 10.0])
    pink = np.fft.irfft(spectrum * scale, n)
    return pink / np.sqrt(np.mean(pink**2))


def _env(n: int, decay: float) -> Floats:
    """An exponential decay over n samples, `decay` seconds to 1/e."""
    return np.exp(-np.arange(n, dtype=np.float64) / (decay * RATE))


def _place(out: Floats, at: float, sound: Floats) -> None:
    """Adds a mono sound to frames x 2 at `at` seconds."""
    start = round(at * RATE)
    if start >= len(out):
        return
    end = min(len(out), start + len(sound))
    out[start:end] += sound[: end - start, None]


# ---------------------------------------------------------------- instruments


def kick(gain: float = 0.8) -> Floats:
    t = _time(0.4)
    freq = 50.0 + 70.0 * np.exp(-t / 0.03)
    phase = 2.0 * np.pi * np.cumsum(freq) / RATE
    return gain * np.sin(phase) * np.exp(-t / 0.25)


def snare(rng: np.random.Generator, gain: float = 0.45) -> Floats:
    t = _time(0.25)
    noise = _noise(rng, len(t))
    noise[1:] -= noise[:-1] * 0.6  # brighter
    body = np.sin(2.0 * np.pi * 180.0 * t) * np.exp(-t / 0.08)
    return gain * (0.7 * noise * np.exp(-t / 0.12) + 0.5 * body)


def hat(rng: np.random.Generator, gain: float = 0.18) -> Floats:
    n = round(0.08 * RATE)
    noise = _noise(rng, n + 1)
    return gain * (noise[1:] - noise[:-1]) * _env(n, 0.025)


def tone(freq: float, seconds: float, gain: float, decay: float, attack: float = 0.005) -> Floats:
    t = _time(seconds)
    rise = np.minimum(1.0, t / attack) if attack > 0 else np.ones_like(t)
    return gain * np.sin(2.0 * np.pi * freq * t) * rise * np.exp(-t / decay)


def piano_note(freq: float, seconds: float, gain: float) -> Floats:
    """A struck note: a few harmonics, the upper ones dying sooner."""
    return sum(
        (tone(freq * h, seconds, gain / h, 0.9 / h, attack=0.002) for h in (1, 2, 3, 4)),
        np.zeros(round(seconds * RATE)),
    )


def strum(freqs: tuple[float, ...], seconds: float, gain: float) -> Floats:
    """A chord struck at once and held, with a short release at the end."""
    t = _time(seconds)
    shape = np.clip(np.minimum(t / 0.01, (seconds - t) / 0.05), 0.0, 1.0) * np.exp(-t / 3.0)
    chord = sum((np.sin(2.0 * np.pi * f * t + i) for i, f in enumerate(freqs)), np.zeros_like(t))
    return gain * chord * shape


def pad(freqs: tuple[float, ...], seconds: float, gain: float) -> Floats:
    """A slow chord with soft attack and release."""
    t = _time(seconds)
    shape = np.minimum(1.0, np.minimum(t / 1.5, (seconds - t) / 1.5))
    chord = sum((np.sin(2.0 * np.pi * f * t + i) for i, f in enumerate(freqs)), np.zeros_like(t))
    return gain * chord * np.clip(shape, 0.0, 1.0)


# ---------------------------------------------------------------- renderer signals


def sweep(seconds: float = 3.0, lo: float = 20.0, hi: float = 20000.0, gain: float = 0.5) -> Floats:
    """A log sine sweep on both channels."""
    t = _time(seconds)
    k = math.log(hi / lo)
    phase = 2.0 * math.pi * lo * seconds / k * (np.exp(t / seconds * k) - 1.0)
    fade = np.clip(np.minimum(t, seconds - t) / 0.05, 0.0, 1.0)
    return _mono(gain * np.sin(phase) * fade)


def kicks(seconds: float = 3.0, bpm: float = 120.0, seed: int = 1) -> Floats:
    """Kicks on the beat and hats in between."""
    rng = np.random.default_rng(seed)
    out = np.zeros((round(seconds * RATE), 2))
    beat = 60.0 / bpm
    for i in range(int(seconds / beat) + 1):
        _place(out, i * beat, kick())
        _place(out, i * beat + beat / 2.0, hat(rng))
    return out


def pink_bursts(seconds: float = 3.0, seed: int = 2) -> Floats:
    """Pink noise stepping through -40, -20 and -8 dBFS, with +12 dB bursts."""
    rng = np.random.default_rng(seed)
    n = round(seconds * RATE)
    noise = _pink(rng, n)
    t = np.arange(n) / RATE
    level = np.where(t < seconds / 3, 0.01, np.where(t < 2 * seconds / 3, 0.1, 0.4))
    bursts = np.where((t % 0.6) < 0.15, 4.0, 1.0)
    return _mono(np.clip(noise * level * bursts * 0.3, -1.0, 1.0))


def silence(seconds: float = 1.0) -> Floats:
    return np.zeros((round(seconds * RATE), 2))


def antiphase(seconds: float = 2.0) -> Floats:
    """Tones with the right channel inverted: a mono mix would cancel them."""
    t = _time(seconds)
    x = (
        0.3 * np.sin(2.0 * np.pi * 100.0 * t)
        + 0.2 * np.sin(2.0 * np.pi * 1000.0 * t)
        + 0.1 * np.sin(2.0 * np.pi * 5000.0 * t)
    ) * (0.5 + 0.5 * np.sin(2.0 * np.pi * 2.0 * t))
    return np.column_stack((x, -x))


def medley(gain: float = 1.0) -> Floats:
    """Sweep, kicks, pink noise bursts, silence and out-of-phase stereo: 8 s."""
    parts = (sweep(2.0), kicks(2.0), pink_bursts(2.0), silence(0.5), antiphase(1.5))
    return np.concatenate(parts) * gain


# ---------------------------------------------------------------- songs


@dataclass
class Song:
    """A synthetic song and its true beats."""

    frames: Capture
    beats: list[float]  # seconds
    bars: list[float]  # first beats of the bars, seconds
    # Spans (seconds) without drums, where no beat can be heard
    breaks: list[tuple[float, float]] = field(default_factory=list[tuple[float, float]])


CHORDS = (
    (220.0, 277.18, 329.63),  # A
    (196.0, 246.94, 293.66),  # G
    (174.61, 220.0, 261.63),  # F
    (164.81, 207.65, 246.94),  # E
)
BASS = (55.0, 49.0, 43.65, 41.2)


def song(
    bpm: float,
    bars: int,
    style: str = 'four',
    seed: int = 3,
    end_bpm: float | None = None,
    breakdown: tuple[int, int] | None = None,
    swing: float = 0.0,
) -> Song:
    """
    Kick, snare, hats, a bass note per beat and a chord per bar. `style`:
    'four' (a kick on every beat) or 'rock' (kicks on 1 and 3, and the "and"
    of 3). `end_bpm` drifts the tempo linearly. `breakdown` (first bar, bar
    after the last) leaves the drums and bass out. `swing` delays every
    second eighth by that share of an eighth.
    """
    rng = np.random.default_rng(seed)
    count = bars * 4
    end = bpm if end_bpm is None else end_bpm
    beats: list[float] = []
    t = 1.0
    for i in range(count):
        beats.append(t)
        tempo = bpm + (end - bpm) * i / max(1, count - 1)
        t += 60.0 / tempo
    length = t + 1.5
    out = np.zeros((round(length * RATE), 2))
    breaks: list[tuple[float, float]] = []
    for i, at in enumerate(beats):
        bar, beat = divmod(i, 4)
        period = (beats[i + 1] if i + 1 < count else t) - at
        if beat == 0:
            _place(out, at, strum(CHORDS[bar % 4], period * 4, 0.06))
        if breakdown is not None and breakdown[0] <= bar < breakdown[1]:
            if beat == 0 and bar == breakdown[0]:
                breaks.append((at, beats[min(count - 1, breakdown[1] * 4)]))
            continue
        # The bass restrikes the bar's note on every beat: only the bar start changes it
        _place(out, at, tone(BASS[bar % 4], period, 0.35, 0.3))
        if style == 'four' or beat in (0, 2):
            _place(out, at, kick())
        if style == 'rock' and beat == 2:
            _place(out, at + period / 2.0, kick(0.6))
        if beat in (1, 3):
            _place(out, at, snare(rng))
        _place(out, at, hat(rng))
        _place(out, at + period / 2.0 * (1.0 + swing), hat(rng, 0.12))
    peak = float(np.max(np.abs(out)))
    frames = capture(out * (0.7 / peak))
    return Song(frames, beats, beats[::4], breaks)


def pads(seconds: float = 20.0, seed: int = 4, noise_db: float | None = -50.0) -> Capture:
    """
    Slow chords that overlap and change at uneven times: no beat. A pink
    noise floor `noise_db` below the peak stands in for a recording's; None
    leaves the sines perfectly clean, which the beat tracker locks onto (a
    known defect, see docs/architecture/current-system.md).
    """
    rng = np.random.default_rng(seed)
    out = np.zeros((round(seconds * RATE), 2))
    at = 0.0
    while at < seconds:
        length = 3.0 + 4.0 * float(rng.random())
        chord = CHORDS[int(rng.random() * 4)]
        _place(out, at, pad(chord, length, 0.12))
        at += 1.3 + 2.9 * float(rng.random())
    out *= 0.6 / float(np.max(np.abs(out)))
    if noise_db is not None:
        out += _mono(_pink(rng, len(out)) * 0.6 * 10.0 ** (noise_db / 20.0))
    return capture(out)


def two_tones(seconds: float = 20.0) -> Capture:
    """A steady, perfectly clean fifth (220 and 330 Hz): the tracker locks onto it."""
    t = _time(seconds)
    x = 0.2 * np.sin(2.0 * np.pi * 220.0 * t) + 0.2 * np.sin(2.0 * np.pi * 329.63 * t)
    return capture(_mono(x))


def piano(seconds: float = 20.0, seed: int = 5) -> Capture:
    """Random notes at random times (about 3 a second): no beat."""
    rng = np.random.default_rng(seed)
    out = np.zeros((round(seconds * RATE), 2))
    at = 0.2
    while at < seconds:
        semitone = int(rng.random() * 36)
        _place(out, at, piano_note(110.0 * 2.0 ** (semitone / 12.0), 1.5, 0.25))
        # exponential gaps, mean 1/3 s
        at += -math.log(1.0 - float(rng.random())) / 3.0 + 0.02
    return capture(out * (0.6 / float(np.max(np.abs(out)))))


def speech(seconds: float = 20.0, seed: int = 6) -> Capture:
    """Bursts of voice-band noise at syllable rate with pauses: no beat."""
    rng = np.random.default_rng(seed)
    n = round(seconds * RATE)
    noise = _pink(rng, n)
    voiced = np.zeros(n)
    t = np.arange(n) / RATE
    at = 0.3
    while at < seconds:
        syllables = 2 + int(rng.random() * 6)
        for _ in range(syllables):
            length = 0.08 + 0.2 * float(rng.random())
            start, stop = round(at * RATE), min(n, round((at + length) * RATE))
            if start >= n:
                break
            k = np.arange(stop - start) / max(1, stop - start - 1)
            voiced[start:stop] = np.sin(np.pi * k) * (0.5 + 0.5 * float(rng.random()))
            at += length + 0.03 + 0.12 * float(rng.random())
        at += 0.3 + 0.9 * float(rng.random())
    pitch = 0.3 * np.sin(2.0 * np.pi * 140.0 * t + 3.0 * np.sin(2.0 * np.pi * 0.7 * t))
    x = (noise * 0.15 + pitch) * voiced
    return capture(_mono(x * (0.6 / float(np.max(np.abs(x))))))


def gap() -> Floats:
    """
    Kicks, 2 s of silence, kicks, 1.5 s of silence: 6 s. Long enough for the
    ambient light to fade in part way, back out when the kicks return, and in again.
    """
    return np.concatenate((kicks(1.0), silence(2.0), kicks(1.5, seed=3), silence(1.5)))


# Signals by name: captured frames ready to be cut into packets
SIGNALS: dict[str, Callable[[], Capture]] = {
    'medley': lambda: capture(medley()),
    'medley_quiet': lambda: capture(medley(0.01)),  # -40 dB: auto level adds the most
    'gap': lambda: capture(gap()),  # silence in the middle: ambient fades in and out
    'song': lambda: song(120.0, 5).frames,  # 12.5 s: about 6 s of beats after the lock
}
