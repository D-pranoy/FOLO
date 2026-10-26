"""
Canonical JSON (RFC 8785, the JSON Canonicalization Scheme) and the item hash
built on it (ADR-0038 D5). `crates/folo-library/src/canonical.rs` gives the
same bytes for the same value; `tests/corpus/canonical.json` checks that they
agree.
"""

from __future__ import annotations

import hashlib
import json
import math
from decimal import Decimal
from typing import Any, cast

Json = Any

_ESCAPES = {
    '"': '\\"',
    '\\': '\\\\',
    '\b': '\\b',
    '\f': '\\f',
    '\n': '\\n',
    '\r': '\\r',
    '\t': '\\t',
}


class NotCanonicalError(ValueError):
    """A value with no canonical form (a number that is not finite)."""


def to_canonical(value: Json) -> str:
    """The value serialised by the JSON Canonicalization Scheme."""
    if value is None:
        return 'null'
    if value is True:
        return 'true'
    if value is False:
        return 'false'
    if isinstance(value, (int, float)):
        return _number(float(value))
    if isinstance(value, str):
        return _string(value)
    if isinstance(value, list):
        items = cast('list[Json]', value)
        return '[' + ','.join(to_canonical(item) for item in items) + ']'
    if isinstance(value, dict):
        mapping = cast('dict[str, Json]', value)
        # UTF-16 code units, which is not the order of UTF-8 bytes beyond the BMP
        keys = sorted(mapping, key=lambda key: key.encode('utf-16-be'))
        return '{' + ','.join(f'{_string(key)}:{to_canonical(mapping[key])}' for key in keys) + '}'
    raise NotCanonicalError(f'{type(value).__name__} is not JSON')


def item_hash(value: Json) -> str:
    """The SHA-256 of the value's canonical JSON, in lowercase hexadecimal."""
    return hashlib.sha256(to_canonical(value).encode('utf-8')).hexdigest()


def _string(text: str) -> str:
    out: list[str] = []
    for char in text:
        if char in _ESCAPES:
            out.append(_ESCAPES[char])
        elif ord(char) < 0x20:
            out.append(f'\\u{ord(char):04x}')
        else:
            out.append(char)
    return '"' + ''.join(out) + '"'


def _number(value: float) -> str:
    """ECMAScript's Number::toString (ECMA-262, 6.1.6.1.20)."""
    if not math.isfinite(value):
        raise NotCanonicalError(f'the number {value} is not finite')
    if value == 0:
        return '0'
    sign = '-' if value < 0 else ''
    # repr() gives the shortest digits that read back as the same number
    _, digit_tuple, exponent = Decimal(repr(abs(value))).normalize().as_tuple()
    digits = ''.join(str(d) for d in digit_tuple)
    k = len(digits)
    n = k + int(exponent)
    return sign + _es_digits(digits, k, n)


def _es_digits(digits: str, k: int, n: int) -> str:
    if k <= n <= 21:
        return digits + '0' * (n - k)
    if 0 < n <= 21:
        return digits[:n] + '.' + digits[n:]
    if -6 < n <= 0:
        return '0.' + '0' * -n + digits
    power = n - 1
    sign = '-' if power < 0 else '+'
    head = digits[0] if k == 1 else digits[0] + '.' + digits[1:]
    return f'{head}e{sign}{abs(power)}'


def loads(text: str) -> Json:
    """JSON text as Python values, refusing `NaN` and `Infinity`."""

    def refuse(name: str) -> Json:
        raise NotCanonicalError(f'{name} is not JSON')

    return json.loads(text, parse_constant=refuse)
