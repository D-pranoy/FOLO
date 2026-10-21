"""
Surface and signal frames (RB-04 step 7, docs/specs/source-link.md): the
layout matches the contract and its fixtures, the pixel formats quantise as
specified (rgb8 gives the golden bytes), LZ4 is used only when smaller, and
resampling averages by area.
"""

from __future__ import annotations

import numpy as np
import pytest
from hypothesis import given
from hypothesis import strategies as st
from hypothesis.extra import numpy as hnp

from folo_source.core import PixelFormat, to_rgbf32
from folo_source.publish import frames
from scripts import conformance

FORMATS: list[PixelFormat] = ['rgb8', 'rgb16', 'rgbf32']


def strip(values: list[list[float]]) -> np.ndarray:
    return np.array(values, dtype=np.float32)


def test_surface_frame_matches_the_fixture() -> None:
    fixture = (conformance.FIXTURES / 'surfaceFrame--rgb8.fols').read_bytes()
    decoded = frames.decode_surface_frame(fixture)
    pixels = decoded.pixels.reshape(-1, 3).astype(np.float32)
    again = frames.surface_frame(
        decoded.index, pixels, 'rgb8', decoded.sequence, decoded.timestamp_us, False
    )
    assert again == fixture


def test_signal_frame_matches_the_fixture() -> None:
    fixture = (conformance.FIXTURES / 'signalFrame--beat.folg').read_bytes()
    decoded = frames.decode_signal_frame(fixture)
    again = frames.signal_frame(
        decoded.index, decoded.values, decoded.sequence, decoded.timestamp_us
    )
    assert again == fixture


@pytest.mark.parametrize('pixel_format', FORMATS)
@pytest.mark.parametrize('lz4', [False, True])
@pytest.mark.parametrize('shape', [(150, 3), (36, 64, 3)])
def test_frames_conform(pixel_format: PixelFormat, lz4: bool, shape: tuple[int, ...]) -> None:
    pixels = np.zeros(shape, dtype=np.float32)  # compresses well
    pixels[..., 0] = 200.0
    data = frames.surface_frame(7, pixels, pixel_format, 3, 123_456, lz4)
    assert conformance.check_frame(data, 'surface') == []
    decoded = frames.decode_surface_frame(data)
    assert decoded.flags & frames.FLAG_KEY_FRAME
    assert bool(decoded.flags & frames.FLAG_LZ4) == lz4
    height = 1 if len(shape) == 2 else shape[0]
    assert decoded.pixels.shape == (height, shape[-2], 3)
    assert (decoded.index, decoded.sequence, decoded.timestamp_us) == (7, 3, 123_456)


def test_lz4_only_when_smaller() -> None:
    noise = np.random.default_rng(1).uniform(0, 255, (8, 3)).astype(np.float32)
    data = frames.surface_frame(0, noise, 'rgb8', 0, 0, True)
    assert not frames.decode_surface_frame(data).flags & frames.FLAG_LZ4
    assert len(data) == frames.SURFACE_HEADER.size + 24


def test_sequence_wraps() -> None:
    data = frames.surface_frame(0, strip([[1, 2, 3]]), 'rgb8', 2**32 + 5, 0, False)
    assert frames.decode_surface_frame(data).sequence == 5


def test_rgb8_truncates_like_todays_engine() -> None:
    wide = np.array([[0.0, 0.999, 1.0], [127.5, 254.99999999, 255.0]])
    payload = frames.quantise(to_rgbf32(wide), 'rgb8')
    assert payload == wide.astype(np.uint8).tobytes()


def test_rgb16_scale() -> None:
    payload = frames.quantise(strip([[0.0, 1.0, 255.0], [0.5, 100.25, 254.999]]), 'rgb16')
    values = np.frombuffer(payload, dtype='<u2').tolist()
    assert values == [0, 257, 65535, 128, 25764, 65534]


def test_rgbf32_keeps_the_values() -> None:
    pixels = strip([[0.0, 12.5, 255.0]])
    assert np.frombuffer(frames.quantise(pixels, 'rgbf32'), dtype='<f4').tolist() == [0, 12.5, 255]


def test_out_of_range_values_are_clipped() -> None:
    payload = frames.quantise(strip([[-3.0, 300.0, 255.0]]), 'rgb8')
    assert list(payload) == [0, 255, 255]


def test_resample_same_size_is_untouched() -> None:
    pixels = strip([[1, 2, 3], [4, 5, 6]])
    assert frames.resample(pixels, 2, 1) is pixels


def test_resample_by_area() -> None:
    pixels = strip([[0, 0, 0], [30, 30, 30], [60, 60, 60], [90, 90, 90]])
    assert frames.resample(pixels, 2, 1)[:, 0].tolist() == [15.0, 75.0]
    # 3 -> 2: the middle cell is split between both
    three = strip([[0, 0, 0], [30, 30, 30], [60, 60, 60]])
    assert frames.resample(three, 2, 1)[:, 0].tolist() == pytest.approx([10.0, 50.0])
    # 2 -> 4: each cell covers two
    two = strip([[10, 10, 10], [20, 20, 20]])
    assert frames.resample(two, 4, 1)[:, 0].tolist() == [10.0, 10.0, 20.0, 20.0]


def test_resample_canvas_and_strip_to_canvas() -> None:
    canvas = np.zeros((4, 4, 3), dtype=np.float32)
    canvas[:2, :2] = 100.0
    out = frames.resample(canvas, 2, 2)
    assert out[..., 0].tolist() == [[100.0, 0.0], [0.0, 0.0]]
    rows = frames.resample(strip([[10, 10, 10], [20, 20, 20]]), 2, 3)
    assert rows.shape == (3, 2, 3)
    assert rows[..., 0].tolist() == [[10.0, 20.0]] * 3


@given(
    hnp.arrays(
        np.float32, st.tuples(st.integers(1, 40), st.just(3)), elements=st.floats(0, 255, width=32)
    ),
    st.integers(1, 60),
)
def test_resample_keeps_the_mean_and_range(pixels: np.ndarray, width: int) -> None:
    out = frames.resample(pixels, width, 1)
    assert out.shape == (width, 3)
    assert np.allclose(out.mean(axis=0), pixels.astype(np.float64).mean(axis=0), atol=1e-3)
    assert out.min() >= pixels.min() - 1e-3 and out.max() <= pixels.max() + 1e-3


@pytest.mark.parametrize(
    'bad',
    [
        b'FOLS',
        b'XXXX' + bytes(24),
        frames.SURFACE_HEADER.pack(b'FOLS', 1, 9, 0, 0, 0, 1, 1, 0, 0, 0) + bytes(3),
    ],
)
def test_bad_surface_frames_are_refused(bad: bytes) -> None:
    with pytest.raises(frames.FrameError):
        frames.decode_surface_frame(bad)


def test_short_payload_is_refused() -> None:
    data = frames.surface_frame(0, strip([[1, 2, 3]]), 'rgb8', 0, 0, False)
    with pytest.raises(frames.FrameError):
        frames.decode_surface_frame(data[:-1])
    with pytest.raises(frames.FrameError):
        frames.decode_signal_frame(frames.signal_frame(0, (1.0, 2.0), 0, 0)[:-1])


def test_a_bad_lz4_payload_is_refused() -> None:
    data = bytearray(frames.surface_frame(0, strip([[1, 2, 3]]), 'rgb8', 0, 0, False))
    data[6] |= frames.FLAG_LZ4
    with pytest.raises(frames.FrameError):
        frames.decode_surface_frame(bytes(data))
