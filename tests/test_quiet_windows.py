"""
The parity gate for quiet windows (RB-04 step 9): while the window is too
quiet to lift any bin above the silence floor, the analyzer skips its FFTs.
What it returns must equal what the FFTs give, at every gain, including the
packets on either side of the bound and fading tails of real sounds.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import cast

import numpy as np
import pytest

from folo_source.inputs.audio.analyzer import Analyzer, Spectrum, channel_peaks
from tests import synth

PACKET = 480


def stereo(mono: np.ndarray) -> np.ndarray:
    return np.column_stack((mono, mono)).astype(np.float32)


def signals() -> dict[str, np.ndarray]:
    """Loud enough first to make the channels count, then quiet and quieter."""
    rng = np.random.default_rng(3)
    start = synth.tone(1000.0, 0.1, 0.5, decay=1e9)
    seconds = 2.0
    n = int(seconds * 48000)
    fade = np.geomspace(1e-4, 1e-11, n)  # down through the bound at every gain
    noise = rng.standard_normal(n) * fade
    tone = np.sin(2.0 * np.pi * 440.0 * np.arange(n) / 48000.0) * fade
    out = {
        'fading-noise': np.concatenate([start, noise]),
        'fading-tone': np.concatenate([start, tone]),
        'digital-silence': np.concatenate([start, np.zeros(n)]),
    }
    return {name: stereo(signal) for name, signal in out.items()}


def spectra(
    signal: np.ndarray, offset: float, shortcut: bool, monkeypatch: pytest.MonkeyPatch
) -> tuple[list[Spectrum], int]:
    """The spectra of every packet, and how many windows took the shortcut."""
    analyzer = Analyzer()
    analyzer.offset = offset
    quiet = cast(Callable[[float], bool], getattr(analyzer, '_quiet'))  # noqa: B009
    taken: list[bool] = []

    def watched(level: float) -> bool:
        skip = bool(quiet(level)) and shortcut  # without the shortcut, every window has FFTs
        taken.append(skip)
        return skip

    monkeypatch.setattr(analyzer, '_quiet', watched)
    out: list[Spectrum] = []
    for at in range(0, len(signal) - PACKET + 1, PACKET):
        frames = signal[at : at + PACKET]
        out.append(analyzer.push(frames, channel_peaks(frames)))
    return out, sum(taken)


@pytest.mark.parametrize('name', list(signals()))
@pytest.mark.parametrize('offset', [-12.0, 0.0, 24.0, 54.0])
def test_quiet_windows_equal_the_ffts(
    name: str, offset: float, monkeypatch: pytest.MonkeyPatch
) -> None:
    signal = signals()[name]
    skipped, taken = spectra(signal, offset, True, monkeypatch)
    computed, none = spectra(signal, offset, False, monkeypatch)
    assert taken > 0 and none == 0
    for packet, (fast, slow) in enumerate(zip(skipped, computed, strict=True)):
        assert np.array_equal(fast.padded_for(False), slow.padded_for(False)), packet
        assert np.array_equal(fast.padded_for(True), slow.padded_for(True)), packet
        assert fast.loudest == slow.loudest, packet
