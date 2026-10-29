"""
The clamper corpus (RB-02 step 4): v2 documents of every kind, mostly
plausible and partly junk, and what folo_source/contracts/clamp.py makes of them.
The node's Rust clamper must give the same outputs
(crates/folo-protocol/tests/corpus.rs). New ids are numbered.

The entries after the generated ones are written by hand (EXTRA_CASES): they
cover what a later change to the schemas added, such as the screen effect and
the screen input's settings (ADR-0037). The generated ones depend on the whole
shape of the schemas, so they are not regenerated for such a change: the
committed file keeps its entries and gets the new ones appended.
"""

from __future__ import annotations

import itertools
import json
from functools import cache
from pathlib import Path
from typing import Any

from hypothesis import HealthCheck, Phase, given, settings

from folo_source.contracts import clamp as c
from tests import schema_strategies

CORPUS = Path(__file__).parent / 'corpus' / 'clamp_v2.json'
SIZE = 300

Json = Any

# Documents for what was added after the generated entries were made, as (definition, input)
EXTRA_CASES: list[tuple[str, Json]] = [
    # ADR-0037: the screen effect's parameters, junk and missing
    (
        'ShowDocument',
        {
            'scenes': [
                {
                    'id': 'scene-screen',
                    'layers_by_role': {
                        'canvas.main': [
                            {
                                'id': 'screen-1',
                                'effect': {'id': 'screen'},
                                'region': {'kind': 'rect', 'x': 0.1, 'y': 0.2, 'width': 0.5},
                                'params': {
                                    'mode': 'wall',
                                    'edge_depth': 9,
                                    'margins': {'top': -1, 'bottom': 0.9, 'left': 'x', 'extra': 1},
                                    'brightness': 99,
                                    'saturation': -2,
                                    'smoothing': 5000,
                                    'bars': 'maybe',
                                    'unknown': True,
                                },
                            },
                            {'id': 'screen-2', 'effect': {'id': 'screen'}, 'params': {}},
                            {
                                'id': 'screen-3',
                                'effect': {'id': 'screen'},
                                'params': {
                                    'mode': 'edge',
                                    'edge_depth': 0.3,
                                    'margins': {
                                        'top': 0.1,
                                        'bottom': 0.05,
                                        'left': 0,
                                        'right': 0.45,
                                    },
                                    'brightness': 1.5,
                                    'saturation': 0,
                                    'smoothing': 0,
                                    'bars': 'ignore',
                                },
                            },
                        ]
                    },
                }
            ]
        },
    ),
    # ADR-0037: the screen input's black bars and profile
    (
        'SourcesDocument',
        {
            'sources': [
                {
                    'id': 'local',
                    'inputs': {
                        'screens': [
                            {},
                            {
                                'id': 'x' * 200,
                                'enabled': True,
                                'black_bars': {'preset': '16:9', 'mode': 'osd', 'threshold': 0.9},
                                'profile': 'game',
                            },
                            {
                                'id': 'display-1',
                                'enabled': True,
                                'black_bars': {
                                    'preset': '21:9',
                                    'mode': 'letterbox',
                                    'threshold': 0,
                                },
                                'profile': 'cinema',
                            },
                            {'id': 'display-2', 'black_bars': 'auto'},
                        ]
                    },
                }
            ]
        },
    ),
    # ADR-0037: the capture agent's restore token in secrets.json
    ('SecretsDocument', {'capture': {'restore_token': 'abc-123'}}),
    ('SecretsDocument', {'capture': {'restore_token': 't' * 300, 'extra': 1}}),
    ('SecretsDocument', {'capture': {}}),
    ('SecretsDocument', {'capture': 5}),
]


def run(definition: str, value: Json) -> Json:
    """The clamped value, or {"error": "structural"}: the clampers must agree on that too."""
    counter = itertools.count(1)
    try:
        return c.clamp(definition, value, lambda: f'new{next(counter):04d}')
    except c.StructuralError:
        return {'error': 'structural'}


def generate(size: int = SIZE) -> list[dict[str, Json]]:
    found: list[tuple[str, Json]] = []

    @settings(
        max_examples=size,
        derandomize=True,
        database=None,
        phases=[Phase.generate],
        suppress_health_check=[
            HealthCheck.too_slow,
            HealthCheck.filter_too_much,
            HealthCheck.data_too_large,
            HealthCheck.large_base_example,
        ],
        deadline=None,
    )
    @given(schema_strategies.documents(finite=True))
    def gather(case: tuple[str, Json]) -> None:
        found.append(case)

    gather()
    entries: list[dict[str, Json]] = []
    for definition, value in found:
        # What is stored is what gets clamped: JSON sorts the keys
        stored = json.loads(json.dumps(value, sort_keys=True))
        entries.append(
            {'definition': definition, 'input': stored, 'output': run(definition, stored)}
        )
    return entries + extra_entries()


def extra_entries() -> list[dict[str, Json]]:
    return [
        {'definition': definition, 'input': stored, 'output': run(definition, stored)}
        for definition, value in EXTRA_CASES
        for stored in [json.loads(json.dumps(value, sort_keys=True))]
    ]


def dumps(entries: list[dict[str, Json]]) -> str:
    return (
        json.dumps({'entries': entries}, separators=(',', ':'), sort_keys=True, ensure_ascii=False)
        + '\n'
    )


@cache
def load() -> list[dict[str, Json]]:
    return json.loads(CORPUS.read_text(encoding='utf-8'))['entries']
