"""
Hypothesis strategies built from the bundled schemas (specs/generated/
folo.schema.json): for any definition, JSON values that are mostly
plausible (in range, out of range, wrong case, unknown keys) and sometimes
junk. They drive the clamper's property tests and its corpus (RB-02 step 4).
"""

from __future__ import annotations

from functools import cache
from typing import Any

from hypothesis import strategies as st

from app.contracts import clamp as clamp_module
from tests import strategies

Json = Any
DOCUMENTS = (
    'ZonesDocument',
    'ShowDocument',
    'ProvidersDocument',
    'SourcesDocument',
    'ClientsDocument',
    'SecretsDocument',
    'NodeSettings',
    'PackManifest',
    'PaletteItem',
    'ReactionItem',
    'LookItem',
    'SceneItem',
    'ZoneTemplateItem',
)


def _defs() -> dict[str, Json]:
    return clamp_module._defs()  # pyright: ignore[reportPrivateUsage]


def _effective(schema: Json) -> Json:
    return clamp_module._effective(schema)  # pyright: ignore[reportPrivateUsage]


def junk(finite: bool) -> st.SearchStrategy[Json]:
    return strategies.json_values(finite)


def _number(schema: Json, finite: bool) -> st.SearchStrategy[Json]:
    lo = schema.get('minimum', -1e6)
    hi = schema.get('maximum', 1e6)
    span = max(1.0, (hi - lo) / 2)
    near = st.floats(lo - span, hi + span, allow_nan=False, allow_infinity=False)
    ints = st.integers(int(lo - span), int(hi + span))
    edges = st.sampled_from([lo, hi, (lo + hi) / 2, lo - 1, hi + 1, 0.5, -0.5, 2.5])
    huge = st.integers(min_value=10**300, max_value=10**400).map(lambda n: n * (1 if n % 2 else -1))
    return st.one_of(near, ints, edges, huge)


def _string(schema: Json) -> st.SearchStrategy[Json]:
    limit = schema.get('maxLength', 64)
    if schema.get('x-folo-color'):
        return st.from_regex(r'#[0-9a-fA-F]{6}', fullmatch=True) | st.sampled_from(
            ['#fff', 'red', '', '#GGGGGG', '#ABCDEF\n', '2700K', '2700k']
        )
    if 'pattern' in schema:
        return st.from_regex(str(schema['pattern'])) | st.text(max_size=8)
    return st.text(max_size=min(limit + 3, 70))


def from_schema(schema: Json, finite: bool = True, depth: int = 0) -> st.SearchStrategy[Json]:
    """Plausible values for a schema, three times out of four; junk otherwise."""
    plausible = _plausible(schema, finite, depth)
    return st.one_of(plausible, plausible, plausible, junk(finite))


def _plausible(schema: Json, finite: bool, depth: int) -> st.SearchStrategy[Json]:
    schema = _effective(schema)
    if depth > 5:
        return junk(finite)
    if 'oneOf' in schema:
        branches = [_plausible(option, finite, depth + 1) for option in schema['oneOf']]
        return st.one_of(*branches)
    if 'const' in schema:
        return st.just(schema['const']) | st.sampled_from(['', 'other'])
    if 'enum' in schema:
        return st.sampled_from(schema['enum']) | st.sampled_from(['', 'other', 1, 'ON'])
    kind = schema.get('type')
    if kind == 'object':
        properties = schema.get('properties')
        extra = schema.get('additionalProperties', True)
        if properties is None:
            if isinstance(extra, dict):
                return st.dictionaries(
                    st.text(max_size=12), from_schema(extra, finite, depth + 1), max_size=3
                )
            # Free-form (effect parameters): sometimes audio-visualizer-shaped
            params = _defs()['AudioVisualizerParams']
            return st.one_of(junk(finite), _plausible(params, finite, depth + 1))
        fields = {name: from_schema(prop, finite, depth + 1) for name, prop in properties.items()}
        return st.fixed_dictionaries({}, optional={**fields, 'unknown_key': junk(finite)})
    if kind == 'array':
        limit = min(schema.get('maxItems', 4) + 2, 5 if depth < 2 else 3)
        return st.lists(from_schema(schema['items'], finite, depth + 1), max_size=limit)
    if kind == 'string':
        return _string(schema)
    if kind in ('number', 'integer'):
        return _number(schema, finite)
    if kind == 'boolean':
        return st.booleans() | st.sampled_from([0, 1, 'true', None])
    return junk(finite)


@cache
def document(name: str, finite: bool = True) -> st.SearchStrategy[Json]:
    """A document's plausible values (objects), for the clamper's entry point."""
    return _plausible(_defs()[name], finite, 0).filter(lambda value: isinstance(value, dict))


def documents(finite: bool = True) -> st.SearchStrategy[tuple[str, Json]]:
    return st.one_of(*[st.tuples(st.just(name), document(name, finite)) for name in DOCUMENTS])
