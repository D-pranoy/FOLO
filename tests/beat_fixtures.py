"""
Beat-tracker fixtures (RB-01 step 6): synthetic songs with known beats and
music without a steady beat, run through the Analyzer and BeatTracker the
way the engine runs them, packet by packet.

`record()` is what the golden files pin: the packets where each beat grid
fired, the bar position at each beat, and the tempo and confidence once a
second. `score()` measures it against the truth.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from folo_source.inputs.audio.analyzer import SAMPLERATE, Analyzer, channel_peaks
from folo_source.signals.beat import BeatTracker
from tests import synth

BEAT_DIR = Path(__file__).parent / 'golden' / 'beat'
GRIDS = (0.5, 1.0, 2.0, 4.0)
TOLERANCE = 0.07  # seconds a fired beat may be off the true one
SETTLE_BARS = 4  # bars after the lock for the bar position to settle
RELOCK = 6.0  # seconds after a breakdown for the beat to come back


@dataclass(frozen=True)
class Fixture:
    name: str
    make: Callable[[], synth.Song | synth.Capture]
    beatless: bool = False  # music without a steady beat: must never lock
    # What the current tracker gets wrong on it, if anything (known defects,
    # docs/architecture/current-system.md). The quality tests expect these to fail.
    beats_defect: str = ''
    bars_defect: str = ''
    lock_defect: str = ''


ROCK_BARS = 'The bar position settles two beats off on a rock pattern (kicks on 1 and 3)'
BREAKDOWN_BARS = 'The bar position comes back one beat off after a breakdown'
CLEAN_LOCK = 'Perfectly clean sustained tones lock the tracker'

FIXTURES = (
    Fixture('four-60', lambda: synth.song(60.0, 6, seed=11)),
    Fixture('four-120', lambda: synth.song(120.0, 12, seed=12)),
    Fixture('four-180', lambda: synth.song(180.0, 16, seed=13)),
    Fixture('rock-140', lambda: synth.song(140.0, 12, 'rock', seed=14), bars_defect=ROCK_BARS),
    Fixture('swing-90', lambda: synth.song(90.0, 8, seed=15, swing=0.33)),
    Fixture('drift-118-126', lambda: synth.song(118.0, 12, seed=16, end_bpm=126.0)),
    Fixture(
        'breakdown-128',
        lambda: synth.song(128.0, 16, seed=17, breakdown=(6, 10)),
        bars_defect=BREAKDOWN_BARS,
    ),
    Fixture('pads', synth.pads, beatless=True),
    Fixture('piano', synth.piano, beatless=True),
    Fixture('speech', synth.speech, beatless=True),
    Fixture('pads-clean', lambda: synth.pads(noise_db=None), beatless=True, lock_defect=CLEAN_LOCK),
    Fixture('two-tones', synth.two_tones, beatless=True, lock_defect=CLEAN_LOCK),
)


@dataclass
class Run:
    song: synth.Song | None
    frames: synth.Capture
    record: dict[str, Any]


def run(fixture: Fixture) -> Run:
    made = fixture.make()
    song = made if isinstance(made, synth.Song) else None
    frames = made.frames if isinstance(made, synth.Song) else made
    analyzer = Analyzer()
    tracker = BeatTracker(analyzer.freqs, SAMPLERATE)
    fired: dict[str, list[int]] = {f'{g:g}': [] for g in GRIDS}
    bar_beats: list[int] = []
    locks: list[int] = []
    bpm: list[float] = []
    confidence: list[float] = []
    locked = False
    for i, packet in enumerate(synth.packets(frames)):
        tracker.push(analyzer.push(packet, channel_peaks(packet)), len(packet))
        if tracker.locked != locked:
            locked = tracker.locked
            locks.append(i)
        for grid in GRIDS:
            if tracker.fired(grid):
                fired[f'{grid:g}'].append(i)
        if tracker.fired(1.0):
            bar_beats.append(tracker.bar_beat)
        if i % 100 == 99:
            bpm.append(round(tracker.bpm, 1))
            confidence.append(round(tracker.confidence, 2))
    record: dict[str, Any] = {
        'signal_sha256': hashlib.sha256(synth.pcm16(frames)).hexdigest(),
        'packets': len(frames) // synth.PACKET,
        'lock_changes': locks,
        'fired': fired,
        'bar_beat': bar_beats,
        'bpm_per_second': bpm,
        'confidence_per_second': confidence,
    }
    if song is not None:
        record['truth'] = {'beats': song.beats, 'bars': song.bars, 'breaks': song.breaks}
    return Run(song, frames, record)


def packet_time(index: int) -> float:
    """When a packet's audio ends, seconds: what the engine has heard by then."""
    return (index + 1) * synth.PACKET / synth.RATE


@dataclass
class Score:
    locked_at: float | None
    beats_off: list[float]  # fired beats more than TOLERANCE from a true beat (seconds)
    beats_missed: list[float]  # true beats after the lock with no fired beat near them
    bars_wrong: list[float]  # settled beats whose bar position isn't the true one


def score(result: Run) -> Score:
    record = result.record
    locks: list[int] = record['lock_changes']
    locked_at = packet_time(locks[0]) if locks else None
    song = result.song
    if song is None or locked_at is None:
        return Score(locked_at, [], [], [])
    beats = np.array(song.beats)
    end = float(beats[-1]) + TOLERANCE
    fired = [packet_time(i) for i in record['fired']['1']]
    playing = [t for t in fired if t <= end]

    def in_break(t: float, after: float = 0.0) -> bool:
        return any(a <= t <= b + after for a, b in song.breaks)

    off = [t for t in playing if float(np.min(np.abs(beats - t))) > TOLERANCE]
    wanted = [float(b) for b in beats if b > locked_at + 0.5 and not in_break(float(b), RELOCK)]
    missed = [b for b in wanted if not fired or min(abs(f - b) for f in fired) > TOLERANCE]
    settled = locked_at + SETTLE_BARS * 4 * float(np.median(np.diff(beats)))
    wrong: list[float] = []
    for t, bar_beat in zip(fired, record['bar_beat'], strict=True):
        if t <= settled or t > end or in_break(t, RELOCK):
            continue
        k = int(np.argmin(np.abs(beats - t)))
        if bar_beat != k % 4:
            wrong.append(t)
    return Score(locked_at, off, missed, wrong)


def golden_path(name: str) -> Path:
    return BEAT_DIR / f'{name}.json'


def dumps(record: dict[str, Any]) -> str:
    return json.dumps(record, indent=1, sort_keys=True) + '\n'
