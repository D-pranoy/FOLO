"""
The sanitize_config corpus (RB-01 step 8): v1-shaped inputs and what
sanitize_config makes of them, for the ports of the clamping rules to check
against (RB-02 step 4). New ids are numbered instead of random while it is
recorded and replayed.
"""

from __future__ import annotations

import copy
import json
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from hypothesis import HealthCheck, Phase, given, settings
from hypothesis import strategies as st

from app import studio_config
from tests import strategies

CORPUS = Path(__file__).parent / 'corpus' / 'sanitize_config.json'
SIZE = 300


@contextmanager
def numbered_ids() -> Generator[None]:
    """new_id() returns new0001, new0002, ... instead of random ids."""
    count = 0
    original = studio_config.new_id

    def numbered() -> str:
        nonlocal count
        count += 1
        return f'new{count:04d}'

    studio_config.new_id = numbered
    try:
        yield
    finally:
        studio_config.new_id = original


def sanitize(cfg: Any, led_count: Any) -> dict[str, Any]:
    with numbered_ids():
        return studio_config.sanitize_config(copy.deepcopy(cfg), led_count)


def generate(size: int = SIZE) -> list[dict[str, Any]]:
    """Inputs from the strategies (JSON-safe, so other languages can read them), derandomized."""
    found: list[tuple[Any, Any]] = []

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
    @given(strategies.inputs(finite=True), st.none() | st.integers(-10, 3000))
    def gather(cfg: Any, led_count: int | None) -> None:
        found.append((copy.deepcopy(cfg), led_count))

    gather()
    # Sanitize what gets stored: the JSON round trip sorts the keys, and str()
    # of a dict depends on their order
    stored = [json.loads(json.dumps(cfg, sort_keys=True)) for cfg, _ in found]
    return [
        {'input': cfg, 'led_count': n, 'output': sanitize(cfg, n)}
        for cfg, (_, n) in zip(stored, found, strict=True)
    ]


def dumps(entries: list[dict[str, Any]]) -> str:
    return (
        json.dumps({'entries': entries}, indent=None, separators=(',', ':'), sort_keys=True) + '\n'
    )


def load() -> list[dict[str, Any]]:
    return json.loads(CORPUS.read_text(encoding='utf-8'))['entries']
