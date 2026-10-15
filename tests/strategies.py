"""
Hypothesis strategies for v1 configs (folo_source/legacy/v1.py): anything JSON
can hold, and config-shaped data whose fields mix plausible values with junk.
"""

from __future__ import annotations

from typing import Any

from hypothesis import strategies as st

from folo_source.legacy import v1 as studio_config

Json = Any


def json_values(finite: bool = False) -> st.SearchStrategy[Json]:
    """What json.loads can return. Python's json also reads NaN and Infinity unless `finite`."""
    scalars = (
        st.none()
        | st.booleans()
        | st.integers()
        | st.text(max_size=12)
        | st.floats(allow_nan=not finite, allow_infinity=not finite)
    )
    return st.recursive(
        scalars,
        lambda children: (
            st.lists(children, max_size=4)
            | st.dictionaries(st.text(max_size=8), children, max_size=4)
        ),
        max_leaves=12,
    )


def _numbers(finite: bool) -> st.SearchStrategy[Json]:
    return (
        st.integers(-(10**6), 10**6)
        | st.floats(-1e6, 1e6, allow_nan=False)
        | st.floats(allow_nan=not finite, allow_infinity=not finite)
        | st.integers()
        | st.from_regex(r'-?[0-9]{1,6}(\.[0-9]{1,3})?', fullmatch=True)
    )


def _field(plausible: st.SearchStrategy[Json], finite: bool) -> st.SearchStrategy[Json]:
    """Mostly plausible values, sometimes anything at all."""
    return st.one_of(plausible, plausible, plausible, json_values(finite))


def _colors() -> st.SearchStrategy[Json]:
    return st.from_regex(r'#[0-9a-fA-F]{6}', fullmatch=True) | st.sampled_from(
        ['#FFF', 'red', '', '#GGGGGG']
    )


def _choices(values: tuple[Any, ...]) -> st.SearchStrategy[Json]:
    return st.sampled_from(values) | st.text(max_size=8)


def _optional(fields: dict[str, st.SearchStrategy[Json]]) -> st.SearchStrategy[dict[str, Json]]:
    return st.fixed_dictionaries({}, optional=fields)


def segments(finite: bool = False) -> st.SearchStrategy[dict[str, Json]]:
    n = _numbers(finite)
    return _optional(
        {
            'id': _field(st.text(max_size=40), finite),
            'name': _field(st.text(max_size=50), finite),
            'enabled': _field(st.booleans(), finite),
            'solo': _field(st.booleans(), finite),
            'start': _field(n, finite),
            'end': _field(n, finite),
            'freq_lo': _field(n, finite),
            'freq_hi': _field(n, finite),
            'trigger': _field(_choices(studio_config.TRIGGERS), finite),
            'beats': _field(st.sampled_from(studio_config.BEAT_DIVISIONS) | n, finite),
            'threshold': _field(n, finite),
            'ceiling': _field(n, finite),
            'gain': _field(n, finite),
            'curve': _field(n, finite),
            'attack': _field(n, finite),
            'release': _field(n, finite),
            'floor': _field(n, finite),
            'mode': _field(_choices(studio_config.MODES), finite),
            'direction': _field(_choices(studio_config.DIRECTIONS), finite),
            'peak': _field(st.booleans(), finite),
            'speed': _field(n, finite),
            'width': _field(n, finite),
            'palette': _field(st.lists(_colors(), max_size=18), finite),
            'lane': _field(n, finite),
        }
    )


def presets(finite: bool = False) -> st.SearchStrategy[dict[str, Json]]:
    return _optional(
        {
            'id': _field(st.text(max_size=70), finite),
            'name': _field(st.text(max_size=50), finite),
            'desc': _field(st.text(max_size=20), finite),
            'tags': _field(st.lists(st.text(max_size=30), max_size=18), finite),
            'led_count': _field(_numbers(finite), finite),
            'segments': _field(st.lists(segments(finite), max_size=3), finite),
        }
    )


def configs(finite: bool = False) -> st.SearchStrategy[dict[str, Json]]:
    n = _numbers(finite)
    return _optional(
        {
            'version': _field(n, finite),
            'led_count': _field(n, finite),
            'audio': _field(
                _optional(
                    {
                        'device': _field(st.text(max_size=20), finite),
                        'app': _field(st.text(max_size=20), finite),
                        'app_mode': _field(_choices(('only', 'except')), finite),
                        'gain': _field(n, finite),
                        'auto_level': _field(st.booleans(), finite),
                    }
                ),
                finite,
            ),
            'hyperion': _field(
                _optional(
                    {
                        'enabled': _field(st.booleans(), finite),
                        'host': _field(st.text(max_size=20), finite),
                        'port': _field(n, finite),
                        'priority': _field(n, finite),
                        'fps': _field(n, finite),
                    }
                ),
                finite,
            ),
            'master': _field(n, finite),
            'background': _field(_colors(), finite),
            'reactive': _field(_choices(studio_config.REACTIVE_MODES), finite),
            'ambient': _field(
                _optional(
                    {
                        'color': _field(_colors(), finite),
                        'style': _field(_choices(studio_config.AMBIENT_STYLES), finite),
                        'level': _field(n, finite),
                        'idle_after': _field(n, finite),
                    }
                ),
                finite,
            ),
            'beat': _field(_optional({'offset': _field(n, finite)}), finite),
            'now_playing': _field(
                _optional(
                    {
                        'enabled': _field(st.booleans(), finite),
                        'cover_colors': _field(st.booleans(), finite),
                        'pause_ambient': _field(st.booleans(), finite),
                    }
                ),
                finite,
            ),
            'segments': _field(st.lists(segments(finite), max_size=6), finite),
            'presets': _field(st.lists(presets(finite), max_size=3), finite),
            'default_preset': _field(st.text(max_size=70), finite),
        }
    )


def inputs(finite: bool = False) -> st.SearchStrategy[Json]:
    """Config-shaped data most of the time, anything at all otherwise."""
    return st.one_of(configs(finite), configs(finite), json_values(finite))
