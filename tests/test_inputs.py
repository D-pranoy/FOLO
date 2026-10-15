"""
The source's inputs (RB-04 step 2): the audio input analyses exactly as the
goldens' reference does, and every input reports in the contracts' shapes.
"""

from __future__ import annotations

import itertools
import sys
from typing import Any

import numpy as np

from folo_source.contracts import models
from folo_source.inputs.audio.analyzer import channel_peaks
from folo_source.inputs.audio.input import AudioInput, AudioPacket
from folo_source.inputs.base import Input
from folo_source.inputs.generator import GeneratorInput
from folo_source.inputs.listing import list_inputs
from folo_source.inputs.media.input import MediaInput
from folo_source.inputs.screen import ScreenInput
from tests import golden_cases, synth


def audio_settings(**changes: Any) -> dict[str, Any]:
    settings: dict[str, Any] = {
        'device': 'default',
        'app': '',
        'app_mode': 'only',
        'gain': 0.0,
        'auto_level': False,
    }
    settings.update(changes)
    return settings


def test_audio_input_analyses_like_the_goldens_reference() -> None:
    """Gain and auto level included: the spectra the golden frames are pinned on."""
    for name, gain, auto_level in (('medley', 6.0, False), ('medley_quiet', 0.0, True)):
        settings = audio_settings(gain=gain, auto_level=auto_level)
        packets: list[AudioPacket] = []
        audio = AudioInput(lambda settings=settings: settings, packets.append)
        for frames in synth.packets(golden_cases.signal(name)):
            audio.on_packet(audio.analyse(frames, channel_peaks(frames), False))
        reference = golden_cases.analyse((name, gain, auto_level))
        assert len(packets) == len(reference)
        for got, want in zip(packets, reference, strict=True):
            assert got.frames == want.frames
            assert got.dt == want.dt
            assert np.array_equal(got.spectrum.padded_for(False), want.spectrum.padded_for(False))
            assert np.array_equal(got.spectrum.padded_for(True), want.spectrum.padded_for(True))


def test_a_new_audio_source_starts_auto_level_afresh() -> None:
    settings = audio_settings(auto_level=True)
    audio = AudioInput(lambda: settings, lambda packet: None)
    for frames in list(synth.packets(golden_cases.signal('medley_quiet')))[:200]:
        audio.analyse(frames, channel_peaks(frames), False)
    assert audio.level.gain > 0.0
    settings['device'] = 'in:Microphone'
    quiet = np.zeros((480, 2), dtype=np.float32)
    audio.analyse(quiet, channel_peaks(quiet), False)
    assert audio.level.gain == 0.0


def test_band_levels_read_the_spectrum() -> None:
    audio = AudioInput(lambda: audio_settings(), lambda packet: None)
    mono = synth.tone(1000.0, 0.2, 0.5, decay=10.0)
    tone = synth.capture(np.column_stack((mono, mono)))
    packet = audio.analyse(tone, channel_peaks(tone), True)
    lows, around, highs = audio.band_levels(packet.spectrum, [40.0, 500.0, 2000.0, 8000.0])
    assert around > lows + 30.0
    assert around > highs + 30.0


def test_inputs_report_in_the_contracts_shapes() -> None:
    audio = AudioInput(lambda: audio_settings(), lambda packet: None)
    models.InputStatus.model_validate(audio.status())
    media = MediaInput(lambda: True, lambda: audio_settings())
    models.MediaState.model_validate(media.status())
    if sys.platform != 'win32':
        assert media.status()['state'] == 'unavailable'
        assert media.health() == 'error'
        assert not media.available
    models.InputList.model_validate(list_inputs(media.available))


def test_every_input_has_the_interface() -> None:
    inputs: list[Input] = [
        AudioInput(lambda: audio_settings(), lambda packet: None),
        MediaInput(lambda: True, lambda: audio_settings()),
        GeneratorInput(),
        ScreenInput('display-1'),
    ]
    assert [i.kind for i in inputs] == ['audio', 'media', 'generator', 'screen']
    for item in inputs:
        assert item.thread is None  # none started
        assert item.health() in ('ok', 'waiting', 'error', 'idle')


def test_generator_lfos() -> None:
    generator = GeneratorInput(start_us=0)
    second = 1_000_000
    assert generator.time(2 * second) == 2.0
    # A 1 Hz cycle, a quarter of the way through
    at = second // 4
    assert abs(generator.lfo(at, 1.0, 'sine') - 0.5) < 1e-9
    assert generator.lfo(at, 1.0, 'triangle') == 0.5
    assert generator.lfo(at, 1.0, 'saw') == 0.25
    assert generator.lfo(at, 1.0, 'square') == 0.0
    assert generator.lfo(0, 1.0, 'sine') == 0.0
    assert generator.lfo(at, 1.0, 'saw', phase=0.5) == 0.75


def test_generator_noise_is_smooth_and_repeatable() -> None:
    first = GeneratorInput(seed=7, start_us=0)
    again = GeneratorInput(seed=7, start_us=0)
    other = GeneratorInput(seed=8, start_us=0)
    times = range(0, 10_000_000, 10_000)  # 10 s every 10 ms
    values = [first.noise(t, 2.0) for t in times]
    assert values == [again.noise(t, 2.0) for t in times]
    assert values != [other.noise(t, 2.0) for t in times]
    assert values != [first.noise(t, 2.0, channel=1) for t in times]
    assert all(0.0 <= v < 1.0 for v in values)
    # 2 values a second, eased: at most a small step every 10 ms
    assert max(abs(b - a) for a, b in itertools.pairwise(values)) < 0.05
    assert max(values) - min(values) > 0.5


def test_screen_inputs_have_no_frames_yet() -> None:
    screen = ScreenInput('display-1', enabled=True)
    assert screen.latest() is None
    assert screen.health() == 'error'
    assert ScreenInput('display-2').health() == 'idle'
