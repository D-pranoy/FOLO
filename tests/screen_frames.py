"""
Screen frames for the tests (docs/specs/source-link.md#screen-frames): the
capture agent's side, which the engine only receives. Everything here is
synthetic; recordings of real screens are never committed.
"""

from __future__ import annotations

import struct
from typing import Any, cast

import numpy as np
import numpy.typing as npt

from folo_source.contracts import clamp
from folo_source.publish.frames import (
    SCREEN_FLAG_HDR,
    SCREEN_FLAG_HELD,
    SCREEN_FLAG_PROTECTED,
    SCREEN_HEADER,
    SCREEN_MAGIC,
    ScreenFrame,
    decode_screen_frame,
)

PROTECTED = SCREEN_FLAG_PROTECTED
HDR = SCREEN_FLAG_HDR
HELD = SCREEN_FLAG_HELD

Pixels = npt.NDArray[np.uint8]


def solid(height: int, width: int, rgb: tuple[int, int, int]) -> Pixels:
    """A picture of one colour."""
    return np.tile(np.array(rgb, dtype=np.uint8), (height, width, 1))


def screen_frame_bytes(
    pixels: Pixels,
    flags: int = 0,
    bars: tuple[int, int, int, int] = (0, 0, 0, 0),
    sequence: int = 0,
    timestamp_us: int = 0,
) -> bytes:
    """A screen frame as the agent sends it (without the message type byte)."""
    height, width = pixels.shape[:2]
    header = SCREEN_HEADER.pack(
        SCREEN_MAGIC, 1, 1, flags, 0, 0, width, height, 0, sequence, timestamp_us, *bars
    )
    return header + pixels.tobytes()


def screen_frame(pixels: Pixels, **kwargs: Any) -> ScreenFrame:
    return decode_screen_frame(screen_frame_bytes(pixels, **kwargs))


def with_header(data: bytes, offset: int, fmt: str, *values: Any) -> bytes:
    """`data` with the header field at `offset` replaced, for malformed frames."""
    patched = bytearray(data)
    struct.pack_into(fmt, patched, offset, *values)
    return bytes(patched)


def canvas_apply(
    layers: list[dict[str, Any]],
    width: int = 8,
    height: int = 4,
    screens: bool = True,
    publish: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """A clamped `config.apply` with one canvas surface (index 0) that has `layers`."""
    params = {
        'revision': 1,
        'inputs': {'screens': [{'id': 'display-1', 'enabled': screens}]},
        'surfaces': [
            {
                'index': 0,
                'surface': {
                    'id': 'tv',
                    'kind': 'canvas',
                    'resolution': {'width': width, 'height': height},
                    'role': 'canvas.main',
                },
                'scene': 'scene',
                'role': 'canvas.main',
            }
        ],
        'scenes': [{'id': 'scene', 'layers_by_role': {'canvas.main': layers}}],
        'publish': publish
        or {
            'surfaces': [
                {
                    'index': 0,
                    'width': width,
                    'height': height,
                    'rate_hz': 200,
                    'pixel_format': 'rgb8',
                    'lz4': False,
                }
            ],
            'signals': [],
            'levels': [],
        },
    }
    return cast(dict[str, Any], clamp.clamp('ConfigApplyParams', params))


def screen_layer(layer_id: str = 's1', **params: Any) -> dict[str, Any]:
    """A Screen layer over the whole canvas."""
    return {
        'id': layer_id,
        'name': 'Screen',
        'effect': {'id': 'screen', 'version': '1.0.0'},
        'params': params,
        'region': {'kind': 'relative', 'start': 0, 'end': 1},
    }
