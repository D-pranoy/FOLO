"""
The clean-room rewrite gate (ADR-0038 D23, D24, docs/specs/effects/README.md):
the table of LedFx-inspired items names exactly the scenes tagged `LedFx` in
the built-in packs and the engine's moving modes, a row past `ported` has its
specification file, and with FOLO_RELEASE_GATE=1, which every publishing
workflow sets, it fails while any row is not `rewritten`.
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path

import pytest

from folo_source.effects.visualizers import MOTION_MODES

ROOT = Path(__file__).resolve().parent.parent
README = ROOT / 'docs' / 'specs' / 'effects' / 'README.md'
EFFECT_SCENES = ROOT / 'packs' / 'folo' / 'effects' / 'scenes'
STATUSES = ('ported', 'specified', 'rewritten')
ROW = re.compile(r'^\| `(?P<id>[a-z0-9-]+)` \|(?P<cells>.*)\|\s*$')
RELEASE_GATE = os.environ.get('FOLO_RELEASE_GATE') == '1'

Row = dict[str, str]


def rows() -> list[Row]:
    """The gate's table: id, specification path and status of each item."""
    found: list[Row] = []
    for line in README.read_text(encoding='utf-8').splitlines():
        match = ROW.match(line)
        if match is None:
            continue
        cells = [c.strip() for c in match.group('cells').split('|')]
        found.append(
            {
                'id': match.group('id'),
                'spec': cells[-2].strip('`'),
                'status': cells[-1],
            }
        )
    return found


def ledfx_scene_ids() -> set[str]:
    """The built-in scenes tagged LedFx."""
    ids: set[str] = set()
    for path in sorted(EFFECT_SCENES.glob('*.json')):
        item = json.loads(path.read_text(encoding='utf-8'))
        if 'LedFx' in item['scene']['tags']:
            ids.add(item['id'])
    return ids


def test_the_table_names_exactly_the_items_to_rewrite() -> None:
    expected = {f'mode-{mode}' for mode in MOTION_MODES} | ledfx_scene_ids()
    listed = [row['id'] for row in rows()]
    assert len(listed) == len(set(listed)), 'an item is listed twice'
    assert set(listed) == expected
    assert len(listed) == 24


def test_every_row_has_a_known_status_and_its_specification_when_past_ported() -> None:
    for row in rows():
        assert row['status'] in STATUSES, row
        assert row['spec'] == f'docs/specs/effects/{row["id"]}.md', row
        if row['status'] != 'ported':
            assert (ROOT / row['spec']).is_file(), (
                f'{row["id"]} is {row["status"]} without its specification'
            )


@pytest.mark.skipif(not RELEASE_GATE, reason='the release gate runs with FOLO_RELEASE_GATE=1')
def test_release_gate_every_item_is_rewritten() -> None:
    open_items = [row['id'] for row in rows() if row['status'] != 'rewritten']
    assert not open_items, f'{len(open_items)} items still to rewrite: {", ".join(open_items)}'
