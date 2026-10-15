"""
The clamper (folo_source/contracts/clamp.py) follows docs/specs/config-model.md's
clamping rules: it never raises on an object, its output is valid against
the schema and accepted by the generated models, and clamping twice changes
nothing. The corpus it shares with the Rust clamper still gives the
recorded outputs (RB-02 step 4).
"""

from __future__ import annotations

import itertools
from functools import cache
from typing import Any

import pytest
from hypothesis import given, settings
from jsonschema import Draft202012Validator

from folo_source.contracts import clamp as c
from folo_source.contracts import models
from tests import clamp_corpus, schema_strategies

Json = Any


def numbered() -> c.NewId:
    counter = itertools.count(1)
    return lambda: f'new{next(counter):04d}'


@cache
def validator(name: str) -> Draft202012Validator:
    return Draft202012Validator({'$ref': f'#/$defs/{name}', '$defs': c._defs()})  # pyright: ignore[reportPrivateUsage]


def clamp_or_structural(name: str, value: Json) -> Json | None:
    """The clamped document, or None when it's from a newer FOLO (the only structural error)."""
    try:
        return c.clamp(name, value, numbered())
    except c.StructuralError:
        version = value.get('schema_version')
        assert isinstance(version, (int, float)) and not isinstance(version, bool) and version > 2
        return None


@settings(max_examples=150)
@given(schema_strategies.documents(finite=False))
def test_clamped_documents_are_valid(case: tuple[str, Json]) -> None:
    name, value = case
    out = clamp_or_structural(name, value)
    if out is None:
        return
    errors = list(validator(name).iter_errors(out))  # pyright: ignore[reportUnknownMemberType]
    assert not errors, errors[0]
    getattr(models, name).model_validate(out)


@settings(max_examples=150)
@given(schema_strategies.documents(finite=False))
def test_idempotent(case: tuple[str, Json]) -> None:
    name, value = case
    once = clamp_or_structural(name, value)
    if once is not None:
        assert c.clamp(name, once, numbered()) == once


def test_structural_errors() -> None:
    with pytest.raises(c.StructuralError):
        c.clamp('ZonesDocument', [])
    with pytest.raises(c.StructuralError):
        c.clamp('ZonesDocument', {'schema_version': 3})
    # An older or missing version is read as the current one
    assert c.clamp('ZonesDocument', {'schema_version': 1})['schema_version'] == 2


def test_audio_visualizer_rules() -> None:
    show = c.clamp(
        'ShowDocument',
        {
            'scenes': [
                {
                    'layers_by_role': {
                        'strip.main': [
                            {
                                'params': {
                                    'freq_lo': 500,
                                    'freq_hi': 200,
                                    'trigger': 'hits',
                                    'threshold': -50,
                                    'ceiling': 2,
                                    'palette': ['#ff0000', 'red'],
                                    'beats': 3,
                                },
                                'region': {'kind': 'cells', 'start': 50, 'end': 10},
                            }
                        ]
                    }
                }
            ]
        },
        numbered(),
    )
    layer = show['scenes'][0]['layers_by_role']['strip.main'][0]
    params = layer['params']
    assert (params['freq_lo'], params['freq_hi']) == (200.0, 500.0)
    assert (params['threshold'], params['ceiling']) == (0.0, 2.0)
    assert params['palette'] == ['#FF0000']
    assert params['beats'] == 1
    assert (layer['region']['start'], layer['region']['end']) == (10, 50)


def test_band_too_narrow_matches_sanitize_segment() -> None:
    from folo_source.legacy.v1 import sanitize_segment

    for lo, hi in ((1000.0, 1000.0), (23990.0, 24000.0), (100.0, 100.5)):
        params = c.clamp('AudioVisualizerParams', {'freq_lo': lo, 'freq_hi': hi})
        legacy = sanitize_segment({'freq_lo': lo, 'freq_hi': hi}, 10)
        assert (params['freq_lo'], params['freq_hi']) == (legacy['freq_lo'], legacy['freq_hi'])


def test_kind_follows_owner() -> None:
    zones = c.clamp(
        'ZonesDocument', {'zones': [{'targets': [{'kind': 'grid', 'geometry': {'kind': 'line'}}]}]}
    )
    assert zones['zones'][0]['targets'][0]['geometry']['kind'] == 'grid'


def test_duplicate_ids_get_new_ones() -> None:
    zones = c.clamp('ZonesDocument', {'zones': [{'id': 'a'}, {'id': 'a'}, {}]}, numbered())
    assert [z['id'] for z in zones['zones']] == ['a', 'new0002', 'new0001']


@pytest.mark.parametrize('index', range(len(clamp_corpus.load())), ids=lambda i: f'entry{i:03d}')
def test_corpus(index: int) -> None:
    entry = clamp_corpus.load()[index]
    assert clamp_corpus.run(entry['definition'], entry['input']) == entry['output']
