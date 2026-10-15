"""
The pipeline's core types (RB-04 step 1, docs/architecture/pipeline.md):
frames, signals, pixel formats and the monotonic microsecond clock.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Literal

import numpy as np
import numpy.typing as npt

# The pixel formats of the contracts (PublishSurface.pixel_format): rgbf32
# inside a process, rgb8 or rgb16 on the wire
PixelFormat = Literal['rgb8', 'rgb16', 'rgbf32']
# A surface frame's pixel format byte (docs/specs/source-link.md#surface-frames)
PIXEL_FORMAT_CODES: dict[PixelFormat, int] = {'rgb8': 1, 'rgb16': 2, 'rgbf32': 3}
BYTES_PER_CHANNEL: dict[PixelFormat, int] = {'rgb8': 1, 'rgb16': 2, 'rgbf32': 4}
# rgbf32 values in the engine are on today's 0-255 scale
FULL_SCALE = 255.0

Pixels = npt.NDArray[np.float32]


def now_us() -> int:
    """The monotonic clock every frame and signal is stamped with, in microseconds."""
    return time.monotonic_ns() // 1000


def to_rgbf32(values: npt.NDArray[np.floating]) -> Pixels:
    """
    64-bit renderer values (0-255) as rgbf32, rounded toward zero: truncating
    the result to rgb8 then gives exactly the bytes truncating the 64-bit
    values does, which keeps parity with today's engine.
    """
    wide = np.asarray(values, dtype=np.float64)
    narrow = wide.astype(np.float32)
    above = narrow > wide
    if above.any():
        narrow[above] = np.nextafter(narrow[above], np.float32(0.0))
    return narrow


@dataclass(frozen=True, slots=True)
class Frame:
    """
    A surface's pixels: rgbf32, shape (N, 3) for a strip or (H, W, 3) for a
    canvas, with the time it was rendered and its sequence number.
    """

    pixels: Pixels
    timestamp_us: int
    sequence: int

    def __post_init__(self) -> None:
        shape = self.pixels.shape
        if self.pixels.dtype != np.float32 or len(shape) not in (2, 3) or shape[-1] != 3:
            raise ValueError(
                f'a frame is rgbf32 (N, 3) or (H, W, 3), not {self.pixels.dtype} {shape}'
            )

    @property
    def kind(self) -> Literal['strip', 'canvas']:
        return 'strip' if self.pixels.ndim == 2 else 'canvas'

    @property
    def width(self) -> int:
        return int(self.pixels.shape[-2])

    @property
    def height(self) -> int:
        return 1 if self.pixels.ndim == 2 else int(self.pixels.shape[0])


@dataclass(frozen=True, slots=True)
class Signal:
    """A named scalar or vector, such as the beat, with its time and sequence number."""

    name: str
    values: tuple[float, ...]
    timestamp_us: int
    sequence: int
