"""
The inputs of the controller's ambient light (RB-08 step 3): what
scripts/make_golden.py --only ambient wrote to tests/golden/ambient/ is what the
engine's reactive frames and activity are today. The Rust tests
(crates/folo-compositor/tests/ambient_golden.rs) apply the zone ambient light to
them and compare with the legacy renderer's frames:

- the five ambient cases of tests/golden/renderer/ (master 1), whose PNGs this
  file only reads;
- four cases with master below 1 and silence in the middle (fade in, fade out
  from part way), whose expected frames the legacy renderer wrote beside their
  inputs.
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
CASES = g.by_name(g.all_ambient_cases())
MASTER_NAMES = [c.name for c in g.master_ambient_cases()]


def test_the_cases_are_the_manifests() -> None:
    assert sorted(MANIFEST['cases']) == sorted(CASES), REGENERATE


def test_the_renderers_ambient_pngs_are_the_expected_frames() -> None:
    renderer = g.load_manifest()['cases']
    for name in g.AMBIENT_NAMES:
        entry = MANIFEST['cases'][name]
        # the expected frames are the renderer's own, never a copy
        assert entry['expected'] == f'renderer/{renderer[name]["png"]}'
        assert (g.GOLDEN_DIR / entry['expected']).is_file()
        assert entry['frames'] == renderer[name]['frames']
        assert entry['master'] == 1.0


def test_the_other_cases_have_master_below_one_and_their_own_expected_frames() -> None:
    for name in MASTER_NAMES:
        entry = MANIFEST['cases'][name]
        assert entry['master'] < 1.0
        assert entry['expected'] == f'ambient/{name}-expected.png'
        assert (g.GOLDEN_DIR / entry['expected']).is_file()


@pytest.mark.parametrize('name', list(CASES))
def test_inputs_unchanged(name: str) -> None:
    case = CASES[name]
    inputs = g.ambient_inputs(case)
    entry = MANIFEST['cases'][name]
    expected = g.ambient_expected_path(case)
    assert json.loads(json.dumps(g.ambient_entry(case, inputs, expected))) == entry, REGENERATE
    stored = g.read_png(g.AMBIENT_DIR / entry['input'])
    assert np.array_equal(stored, inputs.frames), REGENERATE


@pytest.mark.parametrize('name', MASTER_NAMES)
def test_expected_frames_are_the_legacy_renderers(name: str) -> None:
    case = CASES[name]
    entry = MANIFEST['cases'][name]
    stored = g.read_png(g.GOLDEN_DIR / entry['expected'])
    assert np.array_equal(stored, g.render(case).frames), REGENERATE


@pytest.mark.parametrize(
    'name', ['ambient-auto-solid', 'ambient-auto-breathe', 'ambient-master-auto-solid']
)
def test_the_light_leaves_the_inputs_only_after_the_silence_lasts(name: str) -> None:
    """Until the silence lasts `idle_after`, auto mode shows the reactive frames as they are."""
    entry = MANIFEST['cases'][name]
    stored = g.read_png(g.AMBIENT_DIR / entry['input'])
    expected = g.read_png(g.GOLDEN_DIR / entry['expected'])
    assert np.array_equal(stored[:100], expected[:100])
    # and the light does come on later: the expected frames leave the inputs
    assert not np.array_equal(stored, expected)
