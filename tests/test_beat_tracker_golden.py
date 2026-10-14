"""
The beat tracker does on the fixtures exactly what it did when they were
recorded (RB-01 step 8), and how well that is: beats within ±70 ms, bar
starts found, no lock without a steady beat. What the current tracker gets
wrong is listed as known defects (docs/architecture/current-system.md) and
expected to fail until it's fixed.
"""

from __future__ import annotations

import json
from functools import cache

import pytest

from tests import beat_fixtures as bf

REGENERATE = (
    'run scripts/make_golden.py if the change is meant (see docs/process/testing-policy.md)'
)
BY_NAME = {f.name: f for f in bf.FIXTURES}


@cache
def result(name: str) -> bf.Run:
    return bf.run(BY_NAME[name])


def test_fixture_list_matches_goldens() -> None:
    assert sorted(p.stem for p in bf.BEAT_DIR.glob('*.json')) == sorted(BY_NAME), REGENERATE


@pytest.mark.parametrize('name', list(BY_NAME))
def test_record(name: str) -> None:
    expected = json.loads(bf.golden_path(name).read_text(encoding='utf-8'))
    actual = json.loads(bf.dumps(result(name).record))
    assert actual['signal_sha256'] == expected['signal_sha256'], (
        f'fixture {name} changed; {REGENERATE}'
    )
    assert actual == expected, f'the beat tracker behaves differently on {name}; {REGENERATE}'


def with_defect(names: list[str], attr: str) -> list[object]:
    params: list[object] = []
    for name in names:
        defect = getattr(BY_NAME[name], attr)
        marks = [pytest.mark.xfail(strict=True, reason=f'known defect: {defect}')] if defect else []
        params.append(pytest.param(name, marks=marks))
    return params


SONGS = [f.name for f in bf.FIXTURES if not f.beatless]
BEATLESS = [f.name for f in bf.FIXTURES if f.beatless]


@pytest.mark.parametrize('name', with_defect(SONGS, 'beats_defect'))
def test_beats_on_time(name: str) -> None:
    score = bf.score(result(name))
    assert score.locked_at is not None and score.locked_at < 8.0
    assert score.beats_off == [], f'beats more than {bf.TOLERANCE * 1000:.0f} ms off'
    assert score.beats_missed == [], 'true beats without a fired beat'


@pytest.mark.parametrize('name', with_defect(SONGS, 'bars_defect'))
def test_bar_starts(name: str) -> None:
    score = bf.score(result(name))
    assert score.bars_wrong == [], f'{len(score.bars_wrong)} beats with the wrong bar position'


@pytest.mark.parametrize('name', with_defect(BEATLESS, 'lock_defect'))
def test_no_lock_without_a_beat(name: str) -> None:
    assert bf.score(result(name)).locked_at is None
