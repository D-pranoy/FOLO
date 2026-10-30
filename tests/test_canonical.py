"""Canonical JSON (RFC 8785) and item hashes: the shared corpus that the Rust
implementation (crates/folo-library) is checked against too."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from folo_source.contracts.canonical import NotCanonicalError, item_hash, loads, to_canonical

ROOT = Path(__file__).resolve().parent.parent
CORPUS = json.loads((ROOT / 'tests' / 'corpus' / 'canonical.json').read_text(encoding='utf-8'))


@pytest.mark.parametrize('case', CORPUS['cases'], ids=lambda c: c['json'][:40])
def test_corpus_case(case: dict[str, str]) -> None:
    value = loads(case['json'])
    assert to_canonical(value) == case['canonical']
    assert item_hash(value) == case['sha256']


@pytest.mark.parametrize('entry', CORPUS['files'], ids=lambda e: e['path'])
def test_builtin_item_hash(entry: dict[str, str]) -> None:
    value = json.loads((ROOT / entry['path']).read_text(encoding='utf-8'))
    assert item_hash(value) == entry['sha256']


def test_hash_ignores_key_order_and_number_spelling() -> None:
    assert item_hash(loads('{"x": 1.0, "y": "z"}')) == item_hash(loads('{"y":"z","x":1}'))


@pytest.mark.parametrize('text', ['1e999', 'NaN', 'Infinity', '-Infinity'])
def test_non_finite_numbers_have_no_canonical_form(text: str) -> None:
    with pytest.raises(NotCanonicalError):
        to_canonical(loads(text))


def test_huge_integers_and_lone_surrogates_have_no_canonical_form() -> None:
    with pytest.raises(NotCanonicalError):
        to_canonical(10**400)
    with pytest.raises(NotCanonicalError):
        to_canonical('\ud800')
    with pytest.raises(NotCanonicalError):
        to_canonical({'\ud800': 1})
