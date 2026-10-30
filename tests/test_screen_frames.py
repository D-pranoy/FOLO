"""
Screen frames (RB-14 step 7, docs/specs/source-link.md#screen-frames): the
engine's decoder against the contract's fixture, and every way a frame can
be malformed.
"""

from __future__ import annotations

import numpy as np
import pytest

from folo_source.publish.frames import FrameError, decode_screen_frame
from scripts import conformance
from tests import screen_frames as sf


def test_the_fixture_decodes() -> None:
    frame = decode_screen_frame((conformance.FIXTURES / 'screenFrame--rgb8.folc').read_bytes())
    assert frame.pixels.shape == (6, 8, 3)
    assert frame.pixels.dtype == np.uint8
    assert (frame.sequence, frame.timestamp_us, frame.screen_index) == (42, 1_234_567, 0)
    assert frame.bars == (1, 1, 0, 0)
    assert frame.usable and not (frame.held or frame.protected or frame.hdr)


def test_flags() -> None:
    pixels = sf.solid(2, 2, (1, 2, 3))
    assert sf.screen_frame(pixels, flags=sf.HELD).held
    assert not sf.screen_frame(pixels, flags=sf.HELD).usable
    assert sf.screen_frame(pixels, flags=sf.PROTECTED).protected
    assert not sf.screen_frame(pixels, flags=sf.PROTECTED).usable
    hdr = sf.screen_frame(pixels, flags=sf.HDR)
    assert hdr.hdr and hdr.usable


def test_the_pixels_stay_out_of_the_repr() -> None:
    frame = sf.screen_frame(sf.solid(2, 2, (254, 1, 253)))
    assert '254' not in repr(frame) and 'pixels' not in repr(frame)


def test_pixels_are_read_only() -> None:
    frame = sf.screen_frame(sf.solid(2, 2, (9, 9, 9)))
    assert not frame.pixels.flags.writeable


GOOD = sf.screen_frame_bytes(sf.solid(6, 8, (7, 8, 9)), bars=(1, 1, 2, 2), sequence=7)


@pytest.mark.parametrize(
    'data',
    [
        pytest.param(b'', id='empty'),
        pytest.param(GOOD[:20], id='short header'),
        pytest.param(sf.with_header(GOOD, 0, '<4s', b'FOLS'), id='magic'),
        pytest.param(sf.with_header(GOOD, 4, '<B', 2), id='version'),
        pytest.param(sf.with_header(GOOD, 5, '<B', 2), id='pixel format'),
        pytest.param(sf.with_header(GOOD, 6, '<B', 0b1000), id='unknown flag'),
        pytest.param(sf.with_header(GOOD, 10, '<H', 0), id='no width'),
        pytest.param(sf.with_header(GOOD, 12, '<H', 0), id='no height'),
        pytest.param(sf.with_header(GOOD, 10, '<H', 481), id='too wide'),
        pytest.param(sf.with_header(GOOD, 12, '<H', 271), id='too high'),
        pytest.param(GOOD[:-1], id='payload one byte short'),
        pytest.param(GOOD + b'\0', id='payload one byte long'),
        pytest.param(sf.with_header(GOOD, 28, '<HH', 3, 3), id='bars take every row'),
        pytest.param(sf.with_header(GOOD, 32, '<HH', 4, 4), id='bars take every column'),
        pytest.param(sf.with_header(GOOD, 28, '<H', 65535), id='absurd bar'),
    ],
)
def test_malformed_frames_are_refused(data: bytes) -> None:
    with pytest.raises(FrameError) as error:
        decode_screen_frame(data)
    assert '7, 8, 9' not in str(error.value)  # the reason never holds pixels


def test_the_largest_frame_fits_the_limit() -> None:
    frame = sf.screen_frame(sf.solid(270, 480, (1, 1, 1)), bars=(10, 10, 20, 20))
    assert frame.pixels.shape == (270, 480, 3)
    assert len(sf.screen_frame_bytes(frame.pixels)) == 388_836  # the bound in ADR-0037 D2
