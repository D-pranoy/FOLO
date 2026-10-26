"""
The Hyperion golden (RB-10): tests/golden/hyperion/legacy.json holds the bytes the legacy sender
writes and how the studio scales segments to a new LED count, and the Rust provider's tests
compare against it. It must be what the
legacy client writes now, and the commands must be what the studio's documentation says.
"""

from __future__ import annotations

import itertools
import json

from tests import hyperion_wire_cases as cases


def test_the_golden_is_what_the_legacy_client_writes() -> None:
    assert cases.WIRE.read_text(encoding='utf-8') == cases.dumps(cases.generate())


def test_the_commands_are_the_documented_ones() -> None:
    for entry in json.loads(cases.WIRE.read_text(encoding='utf-8'))['cases']:
        color = json.loads(entry['color'])
        assert color['command'] == 'color'
        assert color['color'] == entry['levels']
        assert color['priority'] == entry['priority']
        assert color['origin'] == entry['origin']
        assert color['duration'] == 2000
        assert entry['color'].endswith('\n')
        assert json.loads(entry['clear']) == {'command': 'clear', 'priority': entry['priority']}


def test_the_parity_golden_is_the_legacy_bytes_of_the_renderer_goldens() -> None:
    assert cases.PARITY.read_text(encoding='utf-8') == cases.dumps_parity(cases.generate_parity())


def test_the_parity_frames_change_from_one_to_the_next() -> None:
    for entry in json.loads(cases.PARITY.read_text(encoding='utf-8'))['cases']:
        lines = entry['lines']
        assert len(lines) >= cases.PARITY_FRAMES // 2, entry['png']
        assert all(a != b for a, b in itertools.pairwise(lines)), entry['png']
        assert all('"origin": "Audio Effects"' in line for line in lines)
