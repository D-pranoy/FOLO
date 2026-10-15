"""
The clamper (RB-02 step 4): makes any JSON value a valid FOLO document by
the rules in docs/specs/config-model.md#clamping-rules, driven by the bundled
schemas (specs/generated/folo.schema.json). The node's Rust clamper
(crates/folo-protocol/src/clamp.rs) is a port of this module; a shared corpus
(tests/corpus/clamp_v2.json) checks that they agree.

    clamp('ZonesDocument', data)          # a whole document
    clamp('Zone', data, new_id=...)       # or any definition
"""

from __future__ import annotations

import copy
import json
import math
import re
import uuid
from collections.abc import Callable
from functools import cache
from pathlib import Path
from typing import Any, TypeGuard, cast

BUNDLE = Path(__file__).resolve().parents[3] / 'specs' / 'generated' / 'folo.schema.json'
SCHEMA_VERSION = 2

Json = Any
NewId = Callable[[], str]


class StructuralError(ValueError):
    """The document can't be clamped: its root isn't an object, or it's from a newer FOLO."""


class _Missing:
    """No usable value and no default: the property is left out, the item dropped."""


MISSING = _Missing()


def new_id() -> str:
    return uuid.uuid4().hex[:8]


@cache
def _defs() -> dict[str, Json]:
    return json.loads(BUNDLE.read_text(encoding='utf-8'))['$defs']


def _effective(schema: Json) -> Json:
    """A schema with its $ref followed; keywords beside the $ref (default, x-folo-*) win."""
    while '$ref' in schema:
        target = _defs()[schema['$ref'].removeprefix('#/$defs/')]
        siblings = {k: v for k, v in schema.items() if k != '$ref'}
        schema = {**target, **siblings}
    return schema


def _is_number(value: Json) -> TypeGuard[int | float]:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _obj(value: Json) -> dict[str, Json] | None:
    """A JSON value as an object, or None."""
    return cast('dict[str, Json]', value) if isinstance(value, dict) else None


def _arr(value: Json) -> list[Json] | None:
    """A JSON value as a list, or None."""
    return cast('list[Json]', value) if isinstance(value, list) else None


def _fallback(schema: Json) -> Json:
    return copy.deepcopy(schema['default']) if 'default' in schema else MISSING


def _clamp_number(value: Json, schema: Json) -> Json:
    # Python's json reads NaN and Infinity; huge integers still clamp to the bounds
    if not _is_number(value) or (isinstance(value, float) and not math.isfinite(value)):
        return _fallback(schema)
    if schema['type'] == 'integer':
        # round() ties to even; Python's ints compare exactly with any bound
        number: int | float = round(value) if isinstance(value, float) else value
    else:
        number = value
    if 'minimum' in schema and number < schema['minimum']:
        number = schema['minimum']
    if 'maximum' in schema and number > schema['maximum']:
        number = schema['maximum']
    return int(number) if schema['type'] == 'integer' else float(number)


def _clamp_string(value: Json, schema: Json, ctx: _Context) -> Json:
    pattern = schema.get('pattern')
    if not isinstance(value, str) or (pattern is not None and not re.fullmatch(pattern, value)):
        value = MISSING
    if value is MISSING:
        value = _fallback(schema)
        if value is MISSING:
            return MISSING
    if schema.get('x-folo-color'):
        value = value.upper()
    if 'maxLength' in schema:
        value = value[: schema['maxLength']]
    if schema.get('x-folo-id') and not value:
        value = ctx.new_id()
    return value


def _enum_match(value: Json, options: list[Json]) -> Json:
    for option in options:
        if isinstance(option, str):
            if isinstance(value, str) and value == option:
                return option
        elif _is_number(option) and _is_number(value) and value == option:
            return option
    return MISSING


def _branch(schema: Json, kind: Json) -> Json | None:
    for option in schema['oneOf']:
        branch = _effective(option)
        if branch['properties']['kind']['const'] == kind:
            return branch
    return None


class _Context:
    def __init__(self, make_id: NewId) -> None:
        self.new_id = make_id


def _clamp(schema: Json, value: Json, ctx: _Context) -> Json:
    schema = _effective(schema)
    if 'oneOf' in schema:
        obj = _obj(value)
        branch = _branch(schema, obj.get('kind')) if obj is not None else None
        if branch is None:
            default = _fallback(schema)
            return MISSING if default is MISSING else _clamp(schema, default, ctx)
        return _clamp(branch, value, ctx)
    if 'const' in schema:
        return schema['const']
    if 'enum' in schema:
        matched = _enum_match(value, schema['enum'])
        return matched if matched is not MISSING else _fallback(schema)
    kind = schema.get('type')
    if kind == 'object':
        return _clamp_object(schema, value, ctx)
    if kind == 'array':
        return _clamp_array(schema, value, ctx)
    if kind == 'string':
        return _clamp_string(value, schema, ctx)
    if kind in ('number', 'integer'):
        return _clamp_number(value, schema)
    if kind == 'boolean':
        return value if isinstance(value, bool) else _fallback(schema)
    raise TypeError(f'the clamper does not handle this schema: {schema}')


def _clamp_object(schema: Json, value: Json, ctx: _Context) -> Json:
    obj = _obj(value)
    if obj is None:
        default = _fallback(schema)
        return MISSING if default is MISSING else _clamp_object(schema, default, ctx)
    properties = schema.get('properties')
    extra = schema.get('additionalProperties', True)
    if properties is None:
        if isinstance(extra, dict):
            # A map: every value clamped
            out: dict[str, Json] = {}
            for key, item in obj.items():
                clamped = _clamp(extra, item, ctx)
                if clamped is not MISSING:
                    out[key] = clamped
            return out
        return copy.deepcopy(obj)  # free-form (effect parameters of an unknown effect)
    out = {}
    for name, prop in properties.items():
        prop = _effective(prop)
        raw = obj.get(name, MISSING)
        source = prop.get('x-folo-kind-from')
        if source is not None:
            kind = out.get(source)
            raw_obj = _obj(raw)
            if not (raw_obj is not None and raw_obj.get('kind') == kind):
                raw = {'kind': kind}
        by_effect = prop.get('x-folo-params-by-effect')
        if by_effect is not None:
            effect = out.get('effect', {}).get('id')
            if effect in by_effect:
                prop = {'$ref': by_effect[effect], 'default': dict[str, Json]()}
        if raw is MISSING:
            clamped = (
                _clamp(prop, copy.deepcopy(prop['default']), ctx) if 'default' in prop else MISSING
            )
        else:
            clamped = _clamp(prop, raw, ctx)
            if clamped is MISSING and 'default' in prop:
                clamped = _clamp(prop, copy.deepcopy(prop['default']), ctx)
        if clamped is not MISSING:
            out[name] = clamped
    for rule in schema.get('x-folo-rules', []):
        _RULES[rule](schema, obj, out)
    for name, prop in properties.items():
        decimals = _effective(prop).get('x-folo-decimals')
        if decimals is not None and name in out:
            out[name] = round(out[name], decimals)
    return out


def _clamp_array(schema: Json, value: Json, ctx: _Context) -> Json:
    items = _arr(value)
    if items is None:
        default = _fallback(schema)
        return MISSING if default is MISSING else _clamp_array(schema, default, ctx)
    items_schema = schema['items']
    out: list[Json] = []
    for item in items:
        clamped = _clamp(items_schema, item, ctx)
        if clamped is not MISSING:
            out.append(clamped)
    if 'maxItems' in schema:
        out = out[: schema['maxItems']]
    # Ids are unique within a list
    seen: set[str] = set()
    for item in out:
        entry = _obj(item)
        if entry is not None and _has_id(items_schema):
            if entry['id'] in seen:
                entry['id'] = ctx.new_id()
            seen.add(entry['id'])
    if len(out) < schema.get('minItems', 0):
        default = _fallback(schema)
        return MISSING if default is MISSING else _clamp_array(schema, default, ctx)
    return out


def _has_id(schema: Json) -> bool:
    schema = _effective(schema)
    prop = schema.get('properties', {}).get('id')
    return prop is not None and bool(_effective(prop).get('x-folo-id'))


# ---------------------------------------------------------------- cross-field rules


def _rule_band(schema: Json, raw: Json, out: dict[str, Json]) -> None:
    """freq_lo <= freq_hi, at least the ratio apart (sanitize_segment's rule)."""
    band = schema['x-folo-band']
    low, high, ratio = band['low'], band['high'], band['ratio']
    top = _effective(schema['properties'][high])['maximum']
    lo, hi = out[low], out[high]
    if hi < lo:
        lo, hi = hi, lo
    if hi < lo * ratio:
        hi = min(top, lo * ratio)
        lo = hi / ratio
    out[low], out[high] = lo, hi


def _limited(value: Json, limits: Json) -> float:
    if not _is_number(value) or (isinstance(value, float) and not math.isfinite(value)):
        return float(limits['default'])
    return float(max(limits['minimum'], min(limits['maximum'], value)))


def _rule_thresholds(schema: Json, raw: Json, out: dict[str, Json]) -> None:
    """threshold and ceiling in the units of the trigger, ceiling at least gap above."""
    spec = schema['x-folo-thresholds']
    limits = spec['by_trigger'].get(out[spec['trigger']], spec['otherwise'])
    threshold = _limited(raw.get('threshold'), limits['threshold'])
    ceiling = _limited(raw.get('ceiling'), limits['ceiling'])
    if ceiling < threshold + spec['gap']:
        ceiling = threshold + spec['gap']
    out['threshold'], out['ceiling'] = threshold, ceiling


def _rule_range(schema: Json, raw: Json, out: dict[str, Json]) -> None:
    """start <= end."""
    if out['end'] < out['start']:
        out['start'], out['end'] = out['end'], out['start']


_RULES: dict[str, Callable[[Json, Json, dict[str, Json]], None]] = {
    'band': _rule_band,
    'thresholds': _rule_thresholds,
    'range': _rule_range,
}


# ---------------------------------------------------------------- entry point


def clamp(definition: str, value: Json, new_id: NewId = new_id) -> Json:
    """
    `value` clamped to the schema definition `definition` (a document such as
    ZonesDocument, or any entity). Raises StructuralError for documents that
    can't be clamped.
    """
    schema = _defs()[definition]
    document = _obj(value)
    if document is None:
        raise StructuralError(f'{definition} must be a JSON object')
    version = document.get('schema_version')
    if (
        'schema_version' in schema.get('properties', {})
        and _is_number(version)
        and version > SCHEMA_VERSION
    ):
        raise StructuralError(
            f'{definition} is schema version {version}; this FOLO reads {SCHEMA_VERSION}'
        )
    result = _clamp(schema, document, _Context(new_id))
    assert result is not MISSING
    return result
