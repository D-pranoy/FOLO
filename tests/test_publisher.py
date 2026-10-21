"""
The publisher (RB-04 step 7, docs/architecture/pipeline.md#publishing): each
surface at its requested rate, resolution and pixel format, sequence numbers
that keep counting, and only the signals asked for.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from folo_source.core import Frame, Signal
from folo_source.publish import frames
from folo_source.publish.publisher import Publisher


def publish(rate_hz: float, width: int = 10, height: int = 1, **extra: Any) -> dict[str, Any]:
    surface = {
        'index': 0,
        'width': width,
        'height': height,
        'rate_hz': rate_hz,
        'pixel_format': 'rgb8',
        'lz4': False,
    }
    return {'surfaces': [surface | extra], 'signals': [{'name': 'beat', 'index': 4}]}


def frame(t_us: int, cells: int = 10) -> Frame:
    return Frame(np.full((cells, 3), 100.0, dtype=np.float32), t_us, 0)


def sent_over(publisher: Publisher, seconds: float, step_us: int, cells: int = 10) -> list[int]:
    """The render times (µs) of the frames sent, rendering every step_us."""
    sent: list[int] = []
    for t in range(0, int(seconds * 1_000_000), step_us):
        if publisher.surface(0, frame(t, cells)) is not None:
            sent.append(t)
    return sent


def test_a_lower_rate_is_kept_on_average() -> None:
    publisher = Publisher()
    publisher.configure(publish(60))
    assert len(sent_over(publisher, 1.0, 10_000)) == 60


def test_a_slower_surface_sends_every_frame() -> None:
    publisher = Publisher()
    publisher.configure(publish(100))
    assert len(sent_over(publisher, 1.0, 30_000)) == 34


def test_the_count_starts_again_after_a_pause() -> None:
    publisher = Publisher()
    publisher.configure(publish(10))
    assert publisher.surface(0, frame(0)) is not None
    assert publisher.surface(0, frame(50_000)) is None
    # a pause of more than a period: sent at once, then due a period later
    assert publisher.surface(0, frame(500_000)) is not None
    assert publisher.surface(0, frame(550_000)) is None
    assert publisher.surface(0, frame(600_000)) is not None


def test_sequences_keep_counting_across_configurations() -> None:
    publisher = Publisher()
    publisher.configure(publish(100))
    first = publisher.surface(0, frame(0))
    publisher.configure(publish(100, pixel_format='rgb16'))
    second = publisher.surface(0, frame(20_000))
    assert first is not None and second is not None
    a, b = frames.decode_surface_frame(first), frames.decode_surface_frame(second)
    assert (a.sequence, b.sequence) == (0, 1)
    assert (a.pixel_format, b.pixel_format) == ('rgb8', 'rgb16')


def test_resolution_and_unpublished_surfaces() -> None:
    publisher = Publisher()
    publisher.configure(publish(100, width=5, height=2))
    data = publisher.surface(0, frame(0, cells=10))
    assert data is not None
    decoded = frames.decode_surface_frame(data)
    assert decoded.pixels.shape == (2, 5, 3)
    assert publisher.surface(1, frame(0)) is None
    assert publisher.surfaces == (0,)


def test_signals() -> None:
    publisher = Publisher()
    publisher.configure(publish(100))
    assert publisher.wants('beat') and not publisher.wants('activity')
    assert publisher.signal(Signal('activity', (1.0,), 0, 0)) is None
    data = publisher.signal(Signal('beat', (120.0, 0.5, 1.0, 0.25, 0.0), 77, 0))
    assert data is not None
    decoded = frames.decode_signal_frame(data)
    assert (decoded.index, decoded.sequence, decoded.timestamp_us) == (4, 0, 77)
    assert decoded.values == (120.0, 0.5, 1.0, 0.25, 0.0)
    assert publisher.unknown_signals(['activity']) == ['beat']
