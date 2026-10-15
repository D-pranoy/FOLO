"""
The engine renders the golden cases byte for byte as it did when they were
recorded (RB-01 step 8). After a deliberate change of behaviour, regenerate
them with scripts/make_golden.py and say why in the pull request.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import numpy as np
import pytest

from tests import golden_cases as g

ROOT = Path(__file__).resolve().parent.parent
REGENERATE = (
    'run scripts/make_golden.py if the change is meant (see docs/process/testing-policy.md)'
)

MANIFEST = g.load_manifest()
CASES = g.by_name(g.all_cases())
# Grouped by analysis, so each signal is analysed once
ORDER = [c.name for c in sorted(CASES.values(), key=lambda c: (c.analysis, c.name))]
# Cases that also run through the engine itself (AudioInput.analyse, StudioEngine._on_packet)
ENGINE_CASES = [
    'mode-fill-level-peak',
    'mode-drops-hits',
    'mode-fire-level-floor',
    'mode-wipe-beat-1',
    'gain+24',
    'gain+24-auto-quiet',
    'master-background',
    'beat-offset+100',
]


def test_case_list_matches_manifest() -> None:
    assert sorted(CASES) == sorted(MANIFEST['cases']), REGENERATE


def test_signals_unchanged() -> None:
    for name, sha in MANIFEST['signals'].items():
        assert g.signal_sha256(name) == sha, f'signal {name} changed; {REGENERATE}'


def test_seed_unchanged() -> None:
    assert MANIFEST['seed'] == g.SEED


@pytest.mark.skipif(shutil.which('node') is None, reason='needs Node.js')
def test_builtins_export_is_current() -> None:
    node = shutil.which('node')
    assert node is not None
    out = subprocess.run(
        [node, str(ROOT / 'scripts' / 'export_builtins.mjs')],
        check=True,
        capture_output=True,
        text=True,
        encoding='utf-8',
    ).stdout
    assert json.loads(out) == json.loads(g.BUILTINS.read_text(encoding='utf-8')), (
        'web/studio.js built-ins changed; regenerate with --export-builtins: ' + REGENERATE
    )


def first_difference(actual: g.Frames, expected: g.Frames) -> str:
    if actual.shape != expected.shape:
        return f'shape {actual.shape} != {expected.shape}'
    frame = int(np.flatnonzero((actual != expected).any(axis=(1, 2)))[0])
    leds = np.flatnonzero((actual[frame] != expected[frame]).any(axis=1))
    return (
        f'first difference at frame {frame} ({frame / 100:.2f} s), leds {leds[:8].tolist()}: '
        f'{actual[frame, leds[0]].tolist()} != {expected[frame, leds[0]].tolist()}'
    )


@pytest.mark.parametrize('name', ORDER)
def test_frames(name: str) -> None:
    case = CASES[name]
    entry = MANIFEST['cases'][name]
    result = g.render(case)
    expected = g.read_png(g.RENDERER_DIR / entry['png'])
    if not np.array_equal(result.frames, expected):
        pytest.fail(f'{name}: {first_difference(result.frames, expected)}; {REGENERATE}')
    assert result.frames_sha256() == entry['frames_sha256']
    assert result.levels_sha256() == entry['levels_sha256'], (
        f'{name}: segment levels changed; {REGENERATE}'
    )


@pytest.mark.parametrize('name', ENGINE_CASES)
def test_engine_path(name: str) -> None:
    """render() mirrors the engine: the real path gives the same frames."""
    case = CASES[name]
    expected = g.read_png(g.RENDERER_DIR / MANIFEST['cases'][name]['png'])
    actual = g.render_with_engine(case)
    if not np.array_equal(actual, expected):
        pytest.fail(f'{name}: {first_difference(actual, expected)}')


def test_png_round_trip(tmp_path: Path) -> None:
    rng = np.random.default_rng(0)
    frames = rng.integers(0, 256, size=(17, 5, 3), dtype=np.uint8)
    path = tmp_path / 'frames.png'
    g.write_png(path, frames)
    assert np.array_equal(g.read_png(path), frames)
