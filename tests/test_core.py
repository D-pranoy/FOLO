"""
The pipeline's core types (RB-04 step 1): pixel formats from the contracts,
rgbf32 that truncates to today's bytes, the microsecond clock and frames.
"""

from __future__ import annotations

from typing import get_args

import numpy as np
import pytest

from folo_source import core
from folo_source.contracts import models


def test_pixel_formats_are_the_contracts() -> None:
    contract = models.PublishSurface.model_fields['pixel_format'].annotation
    assert get_args(core.PixelFormat) == get_args(contract)
    assert set(core.PIXEL_FORMAT_CODES) == set(get_args(core.PixelFormat))
    assert core.PIXEL_FORMAT_CODES == {'rgb8': 1, 'rgb16': 2, 'rgbf32': 3}


def test_rgbf32_truncates_to_todays_bytes() -> None:
    rng = np.random.default_rng(3)
    whole = np.arange(256, dtype=np.float64)
    # Just below and above every integer, where rounding to 32 bits would cross it
    near = np.concatenate([whole - 1e-9, whole - 3e-6, whole + 1e-9, whole])
    values = np.clip(np.concatenate([near, rng.uniform(0.0, 255.0, 200_000)]), 0.0, 255.0)
    narrow = core.to_rgbf32(values)
    assert narrow.dtype == np.float32
    assert (narrow.astype(np.float64) <= values).all()
    assert (narrow.astype(np.uint8) == values.astype(np.uint8)).all()


def test_rgbf32_keeps_exact_values() -> None:
    values = np.array([[0.0, 127.0, 255.0], [0.5, 64.25, 200.0]])
    assert (core.to_rgbf32(values) == values.astype(np.float32)).all()


def test_the_clock_is_monotonic_microseconds() -> None:
    first = core.now_us()
    second = core.now_us()
    assert isinstance(first, int)
    assert second >= first


def test_frames() -> None:
    strip = core.Frame(np.zeros((150, 3), dtype=np.float32), timestamp_us=10, sequence=1)
    assert (strip.kind, strip.width, strip.height) == ('strip', 150, 1)
    canvas = core.Frame(np.zeros((36, 64, 3), dtype=np.float32), timestamp_us=10, sequence=2)
    assert (canvas.kind, canvas.width, canvas.height) == ('canvas', 64, 36)
    with pytest.raises(ValueError, match='rgbf32'):
        wrong = np.zeros((150, 3), dtype=np.float64)  # the check is the point
        core.Frame(wrong, timestamp_us=10, sequence=3)  # pyright: ignore[reportArgumentType]
    with pytest.raises(ValueError, match='rgbf32'):
        core.Frame(np.zeros((150, 4), dtype=np.float32), timestamp_us=10, sequence=4)


def test_signals() -> None:
    beat = core.Signal('beat', (120.0, 0.5, 1.0, 0.25, 0.0), timestamp_us=5, sequence=7)
    assert beat.name == 'beat'
    assert len(beat.values) == 5
