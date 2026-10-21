"""
The studio's engine has a frame from the moment it starts: a page that
connects at once must not get an empty one. Rendering without audio only
starts IDLE_AFTER later, so start() renders a first frame itself.
"""

from __future__ import annotations

from typing import Any

import pytest

from folo_source.legacy import v1
from folo_source.runtime.engine import StudioEngine
from tests.golden_cases import FakeStore


def test_the_first_frame_is_there_before_anything_streams(monkeypatch: pytest.MonkeyPatch) -> None:
    config = v1.sanitize_config(v1.default_config(150))
    engine = StudioEngine(FakeStore(config))
    # No capture, players or Hyperion here: only the idle thread runs
    for part in (engine.input, engine.media, engine.sender):
        monkeypatch.setattr(part, 'start', lambda: None)
    try:
        engine.start()
        snapshot: dict[str, Any] = engine.snapshot()
    finally:
        engine.stop()
    assert len(snapshot['leds']) == 6 * config['led_count']
