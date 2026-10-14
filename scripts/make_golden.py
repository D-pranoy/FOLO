"""
Writes the golden files that pin today's engine (RB-01 steps 6 to 8):

- tests/golden/builtins.json: the web UI's built-in looks
  (scripts/export_builtins.mjs, with --export-builtins; needs Node)
- tests/golden/renderer/: rendered frames as PNGs plus manifest.json
- tests/golden/beat/: what the beat tracker does on the fixtures
- tests/corpus/sanitize_config.json: sanitize_config inputs and outputs

    uv run python scripts/make_golden.py [--export-builtins] [--only renderer,beat,corpus]

Regenerate only when behaviour is meant to change, and say why in the pull
request (docs/process/testing-policy.md#golden-files).
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from tests import beat_fixtures, golden_cases, sanitize_corpus  # noqa: E402

PARTS = ('renderer', 'beat', 'corpus')


def export_builtins() -> None:
    node = shutil.which('node')
    if node is None:
        sys.exit('--export-builtins needs Node.js')
    subprocess.run(
        [node, str(ROOT / 'scripts' / 'export_builtins.mjs'), '--out', str(golden_cases.BUILTINS)],
        check=True,
    )
    print(f'wrote {golden_cases.BUILTINS.relative_to(ROOT)}')


def write_renderer() -> None:
    out = golden_cases.RENDERER_DIR
    out.mkdir(parents=True, exist_ok=True)
    cases = golden_cases.all_cases()
    started = time.perf_counter()

    def progress(i: int, total: int, name: str) -> None:
        if i % 25 == 0:
            print(f'  {i}/{total} {name} ({time.perf_counter() - started:.0f} s)', flush=True)

    entries: dict[str, object] = {}
    for case, result in golden_cases.render_all(cases, progress):
        golden_cases.write_png(out / f'{case.name}.png', result.frames)
        entries[case.name] = golden_cases.manifest_entry(case, result)
    names = {f'{c.name}.png' for c in cases}
    for stale in out.glob('*.png'):
        if stale.name not in names:
            stale.unlink()
    manifest = {
        'seed': golden_cases.SEED,
        'signals': {
            name: golden_cases.signal_sha256(name) for name in sorted({c.signal for c in cases})
        },
        'cases': dict(sorted(entries.items())),
    }
    golden_cases.MANIFEST.write_text(
        json.dumps(manifest, indent=1, sort_keys=True) + '\n', encoding='utf-8'
    )
    size = sum(p.stat().st_size for p in out.iterdir())
    print(f'wrote {len(cases)} cases to {out.relative_to(ROOT)} ({size / 1e6:.1f} MB)')


def write_beat() -> None:
    out = beat_fixtures.BEAT_DIR
    out.mkdir(parents=True, exist_ok=True)
    for fixture in beat_fixtures.FIXTURES:
        result = beat_fixtures.run(fixture)
        beat_fixtures.golden_path(fixture.name).write_text(
            beat_fixtures.dumps(result.record), encoding='utf-8'
        )
    names = {f'{f.name}.json' for f in beat_fixtures.FIXTURES}
    for stale in out.glob('*.json'):
        if stale.name not in names:
            stale.unlink()
    print(f'wrote {len(beat_fixtures.FIXTURES)} beat fixtures to {out.relative_to(ROOT)}')


def write_corpus() -> None:
    entries = sanitize_corpus.generate()
    sanitize_corpus.CORPUS.parent.mkdir(parents=True, exist_ok=True)
    sanitize_corpus.CORPUS.write_text(sanitize_corpus.dumps(entries), encoding='utf-8')
    print(f'wrote {len(entries)} entries to {sanitize_corpus.CORPUS.relative_to(ROOT)}')


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        '--export-builtins',
        action='store_true',
        help='export the built-in looks from web/studio.js first (needs Node)',
    )
    parser.add_argument(
        '--only',
        default=','.join(PARTS),
        help=f'comma-separated parts to write (default: {",".join(PARTS)})',
    )
    args = parser.parse_args()
    parts = [p for p in args.only.split(',') if p]
    unknown = set(parts) - set(PARTS)
    if unknown:
        parser.error(f'unknown parts: {", ".join(sorted(unknown))}')
    if args.export_builtins:
        export_builtins()
    if 'renderer' in parts:
        write_renderer()
    if 'beat' in parts:
        write_beat()
    if 'corpus' in parts:
        write_corpus()


if __name__ == '__main__':
    main()
