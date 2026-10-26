"""
The clamper corpus (RB-02 step 4): v2 documents of every kind, mostly
plausible and partly junk, and what folo_source/contracts/clamp.py makes of them.
The node's Rust clamper must give the same outputs
(crates/folo-protocol/tests/corpus.rs). New ids are numbered.

The entries after the generated ones are written by hand (EXTRA_CASES): they
cover what a later change to the schemas added, such as the content library's
fields (ADR-0038). The generated ones depend on the whole shape of the
schemas, so they are not regenerated for such a change: the committed file
keeps its entries and gets the new ones appended.
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
    # ADR-0038 D14: snap on relative regions; D11: provenance's hash and a 65-character pack;
    # D13: remembered bindings; D19: a long default_scene
    (
        'ShowDocument',
        {
            'default_scene': 'folo/genres:' + 'e' * 200,
            'bindings': [
                {'zone': 'default', 'scene': 'folo/genres:edm', 'roles': {'strip.main': 'main'}},
                {
                    'zone': 'z' * 80,
                    'scene': 's' * 300,
                    'roles': {'strip.main': 5, 'canvas.room': ''},
                },
                {'zone': 'default', 'extra': True},
                'junk',
            ],
            'scenes': [
                {
                    'id': 'current',
                    'provenance': {
                        'pack': 'a' * 32 + '/' + 'b' * 32,
                        'version': '1.0.0',
                        'item': 'edm',
                        'hash': '0123456789abcdef' * 4,
                    },
                    'layers_by_role': {
                        'strip.main': [
                            {
                                'id': '1',
                                'region': {
                                    'kind': 'relative',
                                    'start': 0.35,
                                    'end': 0.65,
                                    'snap': 'nearest',
                                },
                            },
                            {
                                'id': '2',
                                'region': {
                                    'kind': 'relative',
                                    'start': 0,
                                    'end': 0.5,
                                    'snap': 'round',
                                },
                            },
                            {
                                'id': '3',
                                'region': {'kind': 'relative', 'end': 0.2, 'snap': 'cover'},
                            },
                        ]
                    },
                },
                {'id': 'other', 'provenance': {'pack': 'folo/core', 'hash': 'ABC'}},
            ],
        },
    ),
    ('ShowDocument', {'bindings': 'none', 'default_scene': 7}),
    # ADR-0038 D17: description and hints on items, categories in the manifest
    (
        'SceneItem',
        {
            'id': 'edm',
            'description': 'd' * 600,
            'hints': {'tempo': 'Fast', 'source': 'LedFx'},
            'scene': {'name': 'EDM festival', 'tags': ['EDM']},
        },
    ),
    ('SceneItem', {'id': 'x', 'description': 5, 'hints': ['not', 'an', 'object']}),
    (
        'ReactionItem',
        {
            'id': 'kick-punch',
            'name': 'Kick punch',
            'description': 'Sharp flash on every kick drum.',
            'hints': {'group': 'Low end', 'anim': 'kick', 'seconds': 0.5},
            'params': {'freq_lo': 40, 'freq_hi': 120, 'trigger': 'hits'},
        },
    ),
    ('PaletteItem', {'id': 'deep-night', 'description': '', 'colors': ['#0033cc', '2700K']}),
    (
        'PackManifest',
        {
            'id': 'folo/core',
            'version': '1.0.0',
            'categories': [
                {'id': 'electronic', 'name': 'Electronic', 'items': ['folo/genres:edm', 'x' * 200]},
                {'name': 'n' * 100, 'description': 'b' * 600, 'items': 'none'},
                3,
            ],
        },
    ),
    ('PackManifest', {'categories': {}}),
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
