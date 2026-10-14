"""
sanitize_config never raises, is idempotent and respects its limits
(RB-01 step 8), and still gives the recorded corpus's outputs.
"""

from __future__ import annotations

import math
import re
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from app import studio_config as sc
from tests import sanitize_corpus, strategies

HEX = re.compile(r'^#[0-9A-F]{6}$')


def check_number(value: Any, lo: float, hi: float, integer: bool = False) -> None:
    assert isinstance(value, int if integer else (int, float)), value
    assert not isinstance(value, bool)
    assert math.isfinite(value)
    assert lo <= value <= hi, (value, lo, hi)


def check_segment(seg: dict[str, Any], led_count: int) -> None:
    assert isinstance(seg['id'], str) and 0 < len(seg['id']) <= 32
    assert isinstance(seg['name'], str) and len(seg['name']) <= 40
    assert isinstance(seg['enabled'], bool) and isinstance(seg['solo'], bool)
    assert isinstance(seg['peak'], bool)
    check_number(seg['start'], 0, led_count - 1, integer=True)
    check_number(seg['end'], seg['start'], led_count - 1, integer=True)
    check_number(seg['freq_lo'], sc.MIN_HZ, sc.MAX_HZ)
    check_number(seg['freq_hi'], seg['freq_lo'], sc.MAX_HZ)
    assert seg['trigger'] in sc.TRIGGERS
    assert seg['beats'] in sc.BEAT_DIVISIONS
    if seg['trigger'] == 'hits':
        check_number(seg['threshold'], 0.0, sc.HITS_MAX_DB - 1.0)
        check_number(seg['ceiling'], 1.0, sc.HITS_MAX_DB)
    else:
        check_number(seg['threshold'], sc.MIN_DB, -1.0)
        check_number(seg['ceiling'], sc.MIN_DB + 1.0, 0.0)
    assert seg['ceiling'] >= seg['threshold'] + 1.0 - 1e-9
    check_number(seg['gain'], 0.1, 8.0)
    check_number(seg['curve'], 0.2, 5.0)
    check_number(seg['attack'], 0.0, 2000.0)
    check_number(seg['release'], 0.0, 5000.0)
    check_number(seg['floor'], 0.0, 1.0)
    assert seg['mode'] in sc.MODES
    assert seg['direction'] in sc.DIRECTIONS
    check_number(seg['speed'], 0.0, sc.MAX_SPEED)
    check_number(seg['width'], 1.0, 100.0)
    assert 1 <= len(seg['palette']) <= 16
    assert all(HEX.match(c) for c in seg['palette'])
    check_number(seg['lane'], -1, sc.MAX_LANES - 1, integer=True)


def check_segments(segments: list[dict[str, Any]], led_count: int) -> None:
    assert len(segments) <= sc.MAX_SEGMENTS
    for seg in segments:
        check_segment(seg, led_count)
    ids = [s['id'] for s in segments]
    assert len(ids) == len(set(ids))


def check_config(cfg: dict[str, Any]) -> None:
    assert cfg['version'] == 1
    check_number(cfg['led_count'], 1, sc.MAX_LEDS, integer=True)
    audio = cfg['audio']
    assert isinstance(audio['device'], str) and 0 < len(audio['device']) <= 300
    assert isinstance(audio['app'], str) and len(audio['app']) <= 260
    assert audio['app'] == audio['app'].strip().lower()
    assert audio['app_mode'] in ('only', 'except')
    check_number(audio['gain'], -sc.MAX_GAIN, sc.MAX_GAIN)
    assert isinstance(audio['auto_level'], bool)
    hyperion = cfg['hyperion']
    assert isinstance(hyperion['enabled'], bool)
    assert isinstance(hyperion['host'], str) and 0 < len(hyperion['host']) <= 255
    check_number(hyperion['port'], 1, 65535, integer=True)
    check_number(hyperion['priority'], 1, 253, integer=True)
    check_number(hyperion['fps'], 5, 60, integer=True)
    check_number(cfg['master'], 0.0, 1.0)
    assert HEX.match(cfg['background'])
    assert cfg['reactive'] in sc.REACTIVE_MODES
    ambient = cfg['ambient']
    assert HEX.match(ambient['color'])
    assert ambient['style'] in sc.AMBIENT_STYLES
    check_number(ambient['level'], 0.0, 1.0)
    check_number(ambient['idle_after'], 1.0, sc.MAX_IDLE_AFTER)
    check_number(cfg['beat']['offset'], -sc.MAX_BEAT_OFFSET, sc.MAX_BEAT_OFFSET, integer=True)
    for key in ('enabled', 'cover_colors', 'pause_ambient'):
        assert isinstance(cfg['now_playing'][key], bool)
    check_segments(cfg['segments'], cfg['led_count'])
    assert len(cfg['presets']) <= sc.MAX_PRESETS
    preset_ids = [p['id'] for p in cfg['presets']]
    assert len(preset_ids) == len(set(preset_ids))
    for preset in cfg['presets']:
        assert isinstance(preset['id'], str) and 0 < len(preset['id']) <= 64
        assert isinstance(preset['name'], str) and 0 < len(preset['name']) <= 40
        assert isinstance(preset['desc'], str) and len(preset['desc']) <= 200
        assert len(preset['tags']) <= sc.MAX_TAGS
        assert all(0 < len(t) <= sc.MAX_TAG_LENGTH for t in preset['tags'])
        assert len({t.lower() for t in preset['tags']}) == len(preset['tags'])
        check_number(preset['led_count'], 1, sc.MAX_LEDS, integer=True)
        check_segments(preset['segments'], preset['led_count'])
    assert isinstance(cfg['default_preset'], str) and len(cfg['default_preset']) <= 64


led_counts = st.none() | strategies.json_values()


@given(strategies.inputs(), led_counts)
def test_never_raises_and_respects_limits(cfg: Any, led_count: Any) -> None:
    check_config(sc.sanitize_config(cfg, led_count))


@given(strategies.inputs(), led_counts)
def test_idempotent(cfg: Any, led_count: Any) -> None:
    once = sc.sanitize_config(cfg, led_count)
    assert sc.sanitize_config(once) == once


def test_huge_integers_fall_back_to_defaults() -> None:
    # Python's json reads integers of any size; float() of one this big overflows
    huge = 10**400
    cfg = sc.sanitize_config({'led_count': huge, 'master': -huge, 'segments': [{'start': huge}]})
    assert cfg['led_count'] == 150
    assert cfg['master'] == 1.0
    assert cfg['segments'][0]['start'] == 0


def test_palette_that_isnt_a_list() -> None:
    # true or a number used to raise; a string's characters and a dict's keys are read as before
    segments = [
        {'palette': True},
        {'palette': 5},
        {'palette': '#FF0000'},
        {'palette': {'#00ff00': 1}},
    ]
    cfg = sc.sanitize_config({'segments': segments})
    assert [s['palette'] for s in cfg['segments']] == [
        ['#FFFFFF'],
        ['#FFFFFF'],
        ['#FFFFFF'],
        ['#00FF00'],
    ]


def corpus_ids() -> list[str]:
    return [f'entry{i:03d}' for i in range(len(sanitize_corpus.load()))]


@pytest.mark.parametrize('index', range(len(sanitize_corpus.load())), ids=corpus_ids())
def test_corpus(index: int) -> None:
    entry = sanitize_corpus.load()[index]
    assert sanitize_corpus.sanitize(entry['input'], entry['led_count']) == entry['output']
