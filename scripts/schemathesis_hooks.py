"""
schemathesis hooks for the conformance harness (scripts/conformance.py), which
loads this file through SCHEMATHESIS_HOOKS.

HTTP clients remove a path's dot segments, `.` and `..` (RFC 3986, section
5.2.4), so a case with a path parameter of `.` or `..` never reaches the node
as generated: `/zones/./deactivate` goes out as `/zones/deactivate`, a path
that names another resource. Such a case tests the client, not the API, and is
left out. Whether ids may be `.` or `..` at all is a question for the contract
(docs/process/merge-train-nits.md).
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

import schemathesis

DOT_SEGMENTS = frozenset({'.', '..'})


def reaches_the_node(path_parameters: Mapping[str, object] | None) -> bool:
    """Whether a URL with these path parameters reaches the node as generated."""
    return all(str(value) not in DOT_SEGMENTS for value in (path_parameters or {}).values())


@schemathesis.hook  # type: ignore[reportUnknownMemberType]  # the decorator is partly untyped
def filter_case(context: schemathesis.HookContext, case: schemathesis.Case[Any]) -> bool:
    del context
    return reaches_the_node(case.path_parameters)
