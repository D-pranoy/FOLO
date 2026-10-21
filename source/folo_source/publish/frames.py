"""
Surface and signal frames on the source link (RB-04 step 7,
docs/specs/source-link.md#surface-frames): resampling to the requested
resolution, quantising to the requested pixel format, the optional LZ4
payload, and the little-endian headers. Decoding is here too, for the
receiving side's tests and tools.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from functools import cache
from typing import cast

import lz4.frame as lz4_frame  # pyright: ignore[reportMissingTypeStubs]
import numpy as np
import numpy.typing as npt

from folo_source.core import BYTES_PER_CHANNEL, FULL_SCALE, PIXEL_FORMAT_CODES, PixelFormat, Pixels

FORMAT_VERSION = 1
SURFACE_MAGIC = b'FOLS'
SIGNAL_MAGIC = b'FOLG'
FLAG_LZ4 = 0b01
FLAG_KEY_FRAME = 0b10
# magic, version, pixel format, flags, reserved, surface index, width, height,
# reserved, sequence, timestamp
SURFACE_HEADER = struct.Struct('<4sBBBBHHHHIQ')
# magic, version, reserved, signal index, count, reserved, sequence, timestamp
SIGNAL_HEADER = struct.Struct('<4sBBHHHIQ')
SEQUENCE_WRAP = 1 << 32
RGB16_SCALE = 257.0  # 255 -> 65535
CHANNELS = 3

_FORMATS_BY_CODE: dict[int, PixelFormat] = {code: fmt for fmt, code in PIXEL_FORMAT_CODES.items()}

Weights = npt.NDArray[np.float64]


class FrameError(ValueError):
    """A frame that doesn't follow the source link's layout."""


def _lz4_compress(data: bytes) -> bytes:
    """An LZ4 frame that records the content size (ADR-0032 D6)."""
    return cast(bytes, lz4_frame.compress(data, store_size=True))  # pyright: ignore[reportUnknownMemberType]


def _lz4_decompress(data: bytes) -> bytes:
    try:
        return cast(bytes, lz4_frame.decompress(data))  # pyright: ignore[reportUnknownMemberType]
    except RuntimeError as error:  # lz4 reports a bad frame this way
        raise FrameError(f'the LZ4 payload is not an LZ4 frame: {error}') from error


@cache
def area_weights(size: int, wanted: int) -> Weights:
    """
    The (wanted, size) matrix that resamples `size` cells to `wanted` by
    area: each wanted cell averages the cells under it, weighted by how
    much of each it covers.
    """
    edges = np.arange(wanted + 1, dtype=np.float64) * (size / wanted)
    lo, hi = edges[:-1, None], edges[1:, None]
    cells = np.arange(size, dtype=np.float64)[None, :]
    covered = np.clip(np.minimum(hi, cells + 1.0) - np.maximum(lo, cells), 0.0, None)
    weights = covered / (size / wanted)
    weights.setflags(write=False)
    return weights


def resample(pixels: Pixels, width: int, height: int) -> Pixels:
    """
    A frame's pixels, (N, 3) or (H, W, 3), at width x height, as (H, W, 3)
    or as (W, 3) when both are strips. The same resolution comes back as it is.
    """
    grid = pixels if pixels.ndim == 3 else pixels[None, :, :]
    rows, cols = grid.shape[0], grid.shape[1]
    if (rows, cols) == (height, width):
        return pixels
    out = grid.astype(np.float64)
    if cols != width:
        out = np.einsum('wc,rcx->rwx', area_weights(cols, width), out)
    if rows != height:
        out = np.einsum('hr,rwx->hwx', area_weights(rows, height), out)
    if pixels.ndim == 2 and height == 1:
        out = out[0]
    return out.astype(np.float32)


def quantise(pixels: Pixels, pixel_format: PixelFormat) -> bytes:
    """
    The payload of a frame: every cell's red, green and blue, row-major, in
    the pixel format. rgb8 truncates toward zero as today's engine does.
    """
    values = np.clip(pixels, 0.0, FULL_SCALE)
    if pixel_format == 'rgb8':
        return values.astype(np.uint8).tobytes()
    if pixel_format == 'rgb16':
        # float32 x 257 is exact in float64, so the truncation is a true floor
        return (values.astype(np.float64) * RGB16_SCALE).astype('<u2').tobytes()
    return values.astype('<f4').tobytes()


def surface_frame(
    index: int,
    pixels: Pixels,
    pixel_format: PixelFormat,
    sequence: int,
    timestamp_us: int,
    lz4: bool,
) -> bytes:
    """
    A surface frame (FOLS) of pixels already at the published resolution.
    With `lz4` the payload is an LZ4 frame, when that is smaller.
    """
    height = 1 if pixels.ndim == 2 else int(pixels.shape[0])
    width = int(pixels.shape[-2])
    payload = quantise(pixels, pixel_format)
    flags = FLAG_KEY_FRAME  # format version 1 has no delta frames
    if lz4:
        packed = _lz4_compress(payload)
        if len(packed) < len(payload):
            payload, flags = packed, flags | FLAG_LZ4
    header = SURFACE_HEADER.pack(
        SURFACE_MAGIC,
        FORMAT_VERSION,
        PIXEL_FORMAT_CODES[pixel_format],
        flags,
        0,
        index,
        width,
        height,
        0,
        sequence % SEQUENCE_WRAP,
        timestamp_us,
    )
    return header + payload


def signal_frame(index: int, values: tuple[float, ...], sequence: int, timestamp_us: int) -> bytes:
    """A signal frame (FOLG): the values as float32."""
    header = SIGNAL_HEADER.pack(
        SIGNAL_MAGIC,
        FORMAT_VERSION,
        0,
        index,
        len(values),
        0,
        sequence % SEQUENCE_WRAP,
        timestamp_us,
    )
    return header + np.asarray(values, dtype='<f4').tobytes()


@dataclass(frozen=True, slots=True)
class SurfaceFrame:
    """A decoded surface frame. `pixels` keeps the payload's own type."""

    index: int
    pixel_format: PixelFormat
    flags: int
    sequence: int
    timestamp_us: int
    pixels: npt.NDArray[np.generic]  # (H, W, 3)


@dataclass(frozen=True, slots=True)
class SignalFrame:
    """A decoded signal frame."""

    index: int
    sequence: int
    timestamp_us: int
    values: tuple[float, ...]


def decode_surface_frame(data: bytes) -> SurfaceFrame:
    """Reads a surface frame, decompressing an LZ4 payload."""
    if len(data) < SURFACE_HEADER.size:
        raise FrameError('shorter than a surface frame header')
    magic, version, code, flags, _, index, width, height, _, sequence, timestamp = (
        SURFACE_HEADER.unpack_from(data)
    )
    if magic != SURFACE_MAGIC or version != FORMAT_VERSION or code not in _FORMATS_BY_CODE:
        raise FrameError(f'not a version {FORMAT_VERSION} surface frame')
    pixel_format = _FORMATS_BY_CODE[code]
    payload = data[SURFACE_HEADER.size :]
    if flags & FLAG_LZ4:
        payload = _lz4_decompress(payload)
    size = width * height * CHANNELS * BYTES_PER_CHANNEL[pixel_format]
    if len(payload) != size:
        raise FrameError(f'payload is {len(payload)} bytes, not {size}')
    dtype = {'rgb8': np.uint8, 'rgb16': np.dtype('<u2'), 'rgbf32': np.dtype('<f4')}[pixel_format]
    pixels = np.frombuffer(payload, dtype=dtype).reshape(height, width, CHANNELS)
    return SurfaceFrame(index, pixel_format, flags, sequence, timestamp, pixels)


def decode_signal_frame(data: bytes) -> SignalFrame:
    """Reads a signal frame."""
    if len(data) < SIGNAL_HEADER.size:
        raise FrameError('shorter than a signal frame header')
    magic, version, _, index, count, _, sequence, timestamp = SIGNAL_HEADER.unpack_from(data)
    if magic != SIGNAL_MAGIC or version != FORMAT_VERSION:
        raise FrameError(f'not a version {FORMAT_VERSION} signal frame')
    payload = data[SIGNAL_HEADER.size :]
    if len(payload) != 4 * count:
        raise FrameError(f'payload is {len(payload)} bytes, not {4 * count}')
    values = tuple(float(v) for v in np.frombuffer(payload, dtype='<f4'))
    return SignalFrame(index, sequence, timestamp, values)
