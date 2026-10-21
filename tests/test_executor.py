"""
The executor (RB-04 step 8) and the parity gate through the whole source
pipeline (RB-04 step 9): today's configs as `config.apply`, clamped like
any the node sends, rendered step by step and published as rgb8 surface
frames, give the RB-01 golden frames byte for byte. Also: skipped steps,
signals, activity and keeping layer state across configurations.
"""

from __future__ import annotations

from typing import Any, cast

import numpy as np
import pytest

from folo_source.contracts import clamp
from folo_source.inputs.audio.analyzer import QUIET_DB, Analyzer, Spectrum
from folo_source.legacy.renderer import layers_from_v1, surface_from_v1
from folo_source.legacy.v1 import default_config, sanitize_config
from folo_source.publish import frames
from folo_source.runtime.executor import Executor, Step
from tests import golden_cases as g
from tests.test_pipeline_parity import CASES, MANIFEST, ORDER

STEP_US = 10_000


class Media:
    def __init__(self, colors: tuple[str, ...] = ()) -> None:
        self.colors = colors
        self.track = 0


def apply_params(
    config: dict[str, Any],
    album_colors: bool = False,
    signals: list[dict[str, Any]] | None = None,
    revision: int = 1,
) -> dict[str, Any]:
    params = {
        'revision': revision,
        'inputs': {},
        'surfaces': [
            {
                'index': 0,
                'surface': surface_from_v1(config),
                'scene': 'scene',
                'role': 'strip.main',
                'beat_offset_ms': int(config['beat']['offset']),
                'album_colors': album_colors,
            }
        ],
        'scenes': [{'id': 'scene', 'layers_by_role': {'strip.main': layers_from_v1(config)}}],
        'publish': {
            'surfaces': [
                {
                    'index': 0,
                    'width': config['led_count'],
                    'height': 1,
                    'rate_hz': 200,
                    'pixel_format': 'rgb8',
                    'lz4': True,
                }
            ],
            'signals': signals or [],
        },
    }
    return cast(dict[str, Any], clamp.clamp('ConfigApplyParams', params))


def make(
    config: dict[str, Any], colors: tuple[str, ...] = (), **extra: Any
) -> tuple[Executor, list[bytes]]:
    sent: list[bytes] = []
    analyzer = Analyzer()
    executor = Executor(analyzer, np.random.default_rng(g.SEED), Media(colors), sent.append)
    executor.apply(apply_params(config, album_colors=bool(colors), **extra))
    return executor, sent


@pytest.mark.parametrize('name', ORDER)
def test_published_frames_match_the_goldens(name: str) -> None:
    case = CASES[name]
    executor, sent = make(case.config, tuple(case.colors))
    t = 0
    for packet in g.analyse(case.analysis):
        t += STEP_US
        executor.step([Step(packet.spectrum, packet.frames, packet.dt)], t)
    for dt in g.idle_steps(case.idle):
        t += STEP_US
        executor.step([Step.silence(dt)], t)
    decoded = [frames.decode_surface_frame(data) for data in sent]
    actual = np.stack([frame.pixels[0] for frame in decoded])
    expected = g.read_png(g.RENDERER_DIR / MANIFEST['cases'][name]['png'])
    assert actual.shape == expected.shape
    differ = np.flatnonzero((actual != expected).any(axis=(1, 2)))
    assert not differ.size, f'{name}: frames differ from frame {int(differ[0])}'
    assert [d.sequence for d in decoded] == list(range(len(decoded)))


def config() -> dict[str, Any]:
    return sanitize_config(default_config(60))


def test_nothing_renders_before_config_apply() -> None:
    sent: list[bytes] = []
    executor = Executor(Analyzer(), np.random.default_rng(0), Media(), sent.append)
    executor.step([Step.silence(0.01)], STEP_US)
    assert not executor.configured and sent == [] and executor.frames == {}


def test_skipped_steps_render_once_with_their_time() -> None:
    executor, sent = make(config())
    executor.step([Step.silence(0.04), Step.silence(0.04), Step.silence(0.04)], STEP_US)
    assert len(sent) == 1
    assert executor.activity.quiet_s == pytest.approx(0.12)


def test_activity() -> None:
    executor, _ = make(config())
    silent = Spectrum.silent()
    padded = silent.padded_for(False).copy()
    padded[100] = QUIET_DB + 10.0
    loud = Spectrum(padded, silent.padded_for(True))
    executor.step([Step(loud, 480, 0.01)], STEP_US)
    assert executor.activity.params() == {
        'active': True,
        'level_db': QUIET_DB + 10.0,
        'quiet_s': 0.0,
    }
    executor.step([Step.silence(0.05)], 2 * STEP_US)
    executor.step([Step.silence(0.05)], 3 * STEP_US)
    assert executor.activity.params() == {'active': False, 'level_db': -120.0, 'quiet_s': 0.1}


def test_signals_are_published_when_asked() -> None:
    signals = [{'name': 'beat', 'index': 0}, {'name': 'activity', 'index': 1}]
    executor, sent = make(config(), signals=signals)
    assert executor.beat_wanted
    executor.step([Step.silence(0.01)], STEP_US)
    decoded = [frames.decode_signal_frame(data) for data in sent if data[:4] == frames.SIGNAL_MAGIC]
    assert [(d.index, len(d.values)) for d in decoded] == [(0, 5), (1, 3)]


def test_layers_keep_their_state_across_configurations() -> None:
    first = config()
    executor, _ = make(first)
    renderer = executor._surfaces[0].renderer  # pyright: ignore[reportPrivateUsage]
    executor.apply(apply_params(first, revision=2))
    assert executor._surfaces[0].renderer is renderer  # pyright: ignore[reportPrivateUsage]
    assert executor.revision == 2


def test_a_new_song_resets_the_beat(monkeypatch: pytest.MonkeyPatch) -> None:
    executor, _ = make(config(), signals=[{'name': 'beat', 'index': 0}])
    resets: list[int] = []
    monkeypatch.setattr(executor.beat, 'reset', lambda: resets.append(1))
    executor.step([Step.silence(0.01)], STEP_US)
    cast(Media, executor.media).track = 1
    executor.step([Step.silence(0.01)], 2 * STEP_US)
    executor.step([Step.silence(0.01)], 3 * STEP_US)
    assert resets == [1]
