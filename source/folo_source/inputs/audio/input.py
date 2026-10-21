"""
The audio input (RB-04 step 2): capture plus analysis. Every batch of new
audio becomes an AudioPacket: its Spectrum (the full and fast windows, in dB
with the input gain and auto level added), how many sample frames it held
and how long it stands for. Levels for any bands come from band_levels().
"""

from __future__ import annotations

import threading
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np
import numpy.typing as npt

from folo_source.core import now_us
from folo_source.inputs.audio.analyzer import SAMPLERATE, Analyzer, AutoLevel, Spectrum
from folo_source.inputs.audio.capture import Capture
from folo_source.inputs.base import InputKind, InputState

MAX_DT = 0.1  # longest step a packet stands for after a gap in the audio

# The capture's states as the source's status notification names them
_STATES: dict[str, InputState] = {'ok': 'ok', 'waiting': 'waiting', 'error': 'error'}


@dataclass(frozen=True, slots=True)
class AudioPacket:
    """One analysed batch of audio."""

    spectrum: Spectrum
    frames: int  # sample frames it held, at SAMPLERATE
    dt: float  # seconds it stands for, at most MAX_DT
    timestamp_us: int


class AudioInput:
    """
    Captures the source in `settings()` (the audio input settings of
    sources.json: `device`, `app`, `app_mode`, `gain`, `auto_level`) and
    hands every analysed packet to `on_packet`, on the capture thread.
    On Linux app capture is unavailable (ADR-0031), and outside Windows and
    Linux capture is; its state and message say so.
    """

    def __init__(
        self,
        settings: Callable[[], Mapping[str, Any]],
        on_packet: Callable[[AudioPacket], None],
    ) -> None:
        self.settings = settings
        self.on_packet = on_packet
        self.analyzer = Analyzer()
        self.level = AutoLevel()
        self._source: tuple[Any, ...] | None = None  # the source auto level has been following
        self.capture = Capture(settings, self._on_audio)

    @property
    def kind(self) -> InputKind:
        return 'audio'

    @property
    def thread(self) -> threading.Thread | None:
        return self.capture.thread

    def start(self) -> None:
        self.capture.start()

    def stop(self) -> None:
        self.capture.stop()

    def health(self) -> InputState:
        return _STATES.get(self.capture.state, 'idle')

    def analyse(
        self,
        frames: npt.NDArray[np.float32],
        peaks: npt.NDArray[np.float32] | None,
        fresh: bool,
    ) -> AudioPacket:
        """
        Analyses a batch of audio: frames x channels, each channel's peak,
        and `fresh` after a gap, when the window starts empty instead of
        mixing in what played before it.
        """
        audio = self.settings()
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
        return AudioPacket(spectrum, len(frames), dt, now_us())

    def band_levels(
        self,
        spectrum: Spectrum,
        edges: Sequence[float] | npt.NDArray[np.float64],
        fast: bool = False,
    ) -> npt.NDArray[np.float64]:
        """The loudest bin between each pair of edges (Hz); the fast window if `fast`."""
        return self.analyzer.band_levels(spectrum, edges, fast)

    def status(self) -> dict[str, Any]:
        """The input notification's fields (live.json's InputStatus)."""
        capture = self.capture
        rate, channels, event = capture.format
        peaks, clip = capture.meter()
        return {
            'state': capture.state,
            'label': capture.label,
            'message': capture.message,
            'rate': rate,
            'channels': channels,
            'event_driven': event,
            'packets_per_s': round(capture.packets, 1),
            'delay_ms': round(capture.delay, 1),
            'glitches': capture.glitches,
            'peaks_db': peaks,
            'clip': clip,
            'gain_db': float(self.settings()['gain']),
            'auto_gain_db': round(self.level.gain, 1),
        }

    def _on_audio(
        self, frames: npt.NDArray[np.float32], peaks: npt.NDArray[np.float32], fresh: bool
    ) -> None:
        self.on_packet(self.analyse(frames, peaks, fresh))
