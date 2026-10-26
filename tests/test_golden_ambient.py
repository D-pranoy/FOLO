"""
The inputs of the controller's ambient light (RB-08 step 3): what
scripts/make_golden.py --only ambient wrote to tests/golden/ambient/ is what the
engine's reactive frames and activity are today. The Rust tests
(crates/folo-compositor/tests/ambient_golden.rs) apply the zone ambient light to
them and compare with tests/golden/renderer/ambient-*.png, which this file
never writes.
"""

from __future__ import annotations

import json

import numpy as np
import pytest

from tests import golden_cases as g

REGENERATE = (
    'run scripts/make_golden.py --only ambient if the change is meant '
    '(see docs/process/testing-policy.md)'
)

MANIFEST = json.loads(g.AMBIENT_MANIFEST.read_text(encoding='utf-8'))
CASES = g.by_name(g.ambient_cases())


def test_the_ambient_cases_are_the_renderers_ambient_pngs() -> None:
    assert sorted(MANIFEST['cases']) == sorted(g.AMBIENT_NAMES), REGENERATE
    renderer = g.load_manifest()['cases']
    for name, entry in MANIFEST['cases'].items():
        # the expected frames are the renderer's own, never a copy
        assert entry['expected'] == f'renderer/{name}.png'
        assert entry['expected'].split('/', 1)[1] == renderer[name]['png']
        assert (g.GOLDEN_DIR / entry['expected']).is_file()
        assert (g.AMBIENT_DIR / entry['input']).is_file()
        assert entry['frames'] == renderer[name]['frames']


@pytest.mark.parametrize('name', g.AMBIENT_NAMES)
def test_inputs_unchanged(name: str) -> None:
    case = CASES[name]
    inputs = g.ambient_inputs(case)
    entry = MANIFEST['cases'][name]
    assert json.loads(json.dumps(g.ambient_entry(case, inputs))) == entry, REGENERATE
    stored = g.read_png(g.AMBIENT_DIR / entry['input'])
    assert np.array_equal(stored, inputs.frames), REGENERATE


@pytest.mark.parametrize('name', ['ambient-auto-solid', 'ambient-auto-breathe'])
def test_inputs_are_the_expected_frames_before_the_light_comes_on(name: str) -> None:
    """Until the silence lasts `idle_after`, auto mode shows the reactive frames as they are."""
    entry = MANIFEST['cases'][name]
    stored = g.read_png(g.AMBIENT_DIR / entry['input'])
    expected = g.read_png(g.GOLDEN_DIR / entry['expected'])
    assert np.array_equal(stored[:100], expected[:100])
    # and the light does come on later: the expected frames leave the inputs
    assert not np.array_equal(stored, expected)
