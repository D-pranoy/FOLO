"""
The beat signal (RB-04 step 3): a tracker per zone, each exactly today's
tracker with its zone's offset, and the values the signal publishes.
"""

from __future__ import annotations

from folo_source.inputs.audio.analyzer import SAMPLERATE, Analyzer, channel_peaks
from folo_source.signals.beat import ONSET_DELAY, BeatSignal, BeatTracker, beat_values
from tests import synth

GRIDS = (0.5, 1.0, 2.0, 4.0)


def test_each_zone_tracks_like_todays_tracker_with_its_offset() -> None:
    frames = synth.song(120.0, 8, seed=21).frames
    analyzer = Analyzer()
    beat = BeatSignal(analyzer.freqs, SAMPLERATE)
    beat.keep({'early': 80.0, 'late': -40.0})
    early = BeatTracker(analyzer.freqs, SAMPLERATE)
    early.lead = ONSET_DELAY + 0.08
    late = BeatTracker(analyzer.freqs, SAMPLERATE)
    late.lead = ONSET_DELAY - 0.04
    fired = 0
    for packet in synth.packets(frames):
        spectrum = analyzer.push(packet, channel_peaks(packet))
        beat.push(spectrum, len(packet))
        early.push(spectrum, len(packet))
        late.push(spectrum, len(packet))
        for key, reference in (('early', early), ('late', late)):
            tracker = beat.trackers[key]
            assert beat_values(tracker) == beat_values(reference)
            assert [tracker.fired(g) for g in GRIDS] == [reference.fired(g) for g in GRIDS]
        fired += beat.trackers['early'].fired(1.0)
    assert fired > 8, 'the song locks and beats fire'
    assert beat.values() == beat_values(beat.trackers['early'])


def test_a_new_offset_keeps_the_lock() -> None:
    frames = synth.song(120.0, 6, seed=22).frames
    analyzer = Analyzer()
    beat = BeatSignal(analyzer.freqs, SAMPLERATE)
    beat.keep({'zone': 0.0})
    tracker = beat.trackers['zone']
    for packet in synth.packets(frames):
        beat.push(analyzer.push(packet, channel_peaks(packet)), len(packet))
        if tracker.locked:
            break
    assert tracker.locked
    beat.keep({'zone': 120.0, 'other': 0.0})
    assert beat.trackers['zone'] is tracker
    assert tracker.locked
    assert tracker.lead == ONSET_DELAY + 0.12
    assert not beat.trackers['other'].locked
    beat.keep({'other': 0.0})
    assert list(beat.trackers) == ['other']


def test_values_without_a_zone() -> None:
    beat = BeatSignal(Analyzer().freqs, SAMPLERATE)
    assert beat.values() == (0.0, 0.0, 0.0, 0.0, 0.0)
