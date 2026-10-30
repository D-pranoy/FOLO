"""
What the legacy Hyperion output does, for the Rust provider to match (RB-10): the bytes the
sender writes (the `color` command of frames of several sizes, priorities and origins, and the
`clear` command it writes when it lets go, captured from
`folo_source.legacy.json_client.JsonClient` itself), and how the studio stretches segments when
Hyperion's LED count changes (`scale_segments`). The Rust provider's tests compare against
tests/golden/hyperion/legacy.json (scripts/make_golden.py --only hyperion).
"""

from __future__ import annotations

import json
from typing import Any

from folo_source.legacy import json_client, v1
from folo_source.legacy.hyperion import HYPERION_DURATION
from tests import golden_cases

WIRE = golden_cases.GOLDEN_DIR / 'hyperion' / 'legacy.json'

# (name, priority, origin, levels): the origin goes through json.dumps with ensure_ascii
CASES: list[tuple[str, int, str, list[int]]] = [
    ('one dark led', 100, 'FOLO', [0, 0, 0]),
    ('three leds', 100, 'FOLO', [0, 255, 7, 255, 0, 128, 1, 2, 3]),
    ('the legacy origin', 100, 'Audio Effects', [10, 20, 30, 40, 50, 60]),
    ('lowest and highest priority', 1, 'FOLO', [255, 255, 255]),
    ('highest priority', 253, 'FOLO', [255, 255, 255]),
    ('150 leds', 100, 'FOLO', [(i * 7 + c * 83) % 256 for i in range(150) for c in range(3)]),
    ('an origin that needs escapes', 100, 'a"b\\cü☃\U0001f600', [9, 8, 7]),
]


class RecordingSocket:
    """What JsonClient writes to, keeping the bytes."""

    def __init__(self) -> None:
        self.sent: list[bytes] = []

    def sendall(self, data: bytes) -> None:
        self.sent.append(data)

    def close(self) -> None:
        pass


def legacy_bytes(priority: int, origin: str, levels: list[int]) -> tuple[str, str]:
    """The `color` line for one frame and the `clear` line, as the legacy client writes them."""
    client = json_client.JsonClient(
        '127.0.0.1', 19444, priority=priority, duration=HYPERION_DURATION
    )
    sock = RecordingSocket()
    client.socket = sock  # type: ignore[assignment]
    client.connected = True
    saved = json_client.ORIGIN
    json_client.ORIGIN = origin
    try:
        client.send_led_data(bytearray(levels))  # pyright: ignore[reportUnknownMemberType]
    finally:
        json_client.ORIGIN = saved
    client.disconnect()
    color, clear = sock.sent
    return color.decode('ascii'), clear.decode('ascii')


# (start, end, old LED count, new LED count): whole strips, thirds, single LEDs, ends, and the
# halves that Python's rounding sends to the even number
SCALING: list[tuple[int, int, int, int]] = (
    [(0, 149, 150, 300), (0, 49, 150, 300), (50, 99, 150, 300), (100, 149, 150, 75)]
    + [(5, 5, 10, 5), (3, 3, 10, 5), (7, 7, 10, 5), (9, 9, 10, 5), (0, 0, 10, 5)]
    + [
        (a, b, old, new)
        for old, new in ((60, 90), (90, 60), (144, 150), (30, 29), (7, 100))
        for a, b in ((0, old - 1), (0, 0), (old // 3, old // 2), (old - 1, old - 1))
    ]
)


def scaled(start: int, end: int, old: int, new: int) -> tuple[int, int]:
    """The segment as the studio's scale_segments leaves it."""
    config = {'led_count': old, 'segments': [{'start': start, 'end': end}]}
    segment = v1.scale_segments(config, new)['segments'][0]
    return segment['start'], segment['end']


def generate() -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for name, priority, origin, levels in CASES:
        color, clear = legacy_bytes(priority, origin, levels)
        entries.append(
            {
                'name': name,
                'priority': priority,
                'origin': origin,
                'levels': levels,
                'color': color,
                'clear': clear,
            }
        )
    return entries


def generate_scaling() -> list[list[int]]:
    return [[*case, *scaled(*case)] for case in SCALING]


def dumps(entries: list[dict[str, Any]]) -> str:
    document = {'cases': entries, 'scaling': generate_scaling()}
    return json.dumps(document, indent=2) + '\n'


def write() -> int:
    entries = generate()
    WIRE.parent.mkdir(parents=True, exist_ok=True)
    WIRE.write_text(dumps(entries), encoding='utf-8')
    return len(entries)
