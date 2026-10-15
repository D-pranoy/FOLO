"""
The screen input (RB-04 step 2): a screen captured by the capture agent,
folo-capture, which hands its frames to the effect engine. The agent comes
in RB-14 (docs/runbooks/m6-screen.md); until then a screen input has no
frames.
"""

from __future__ import annotations

import threading
from typing import Any

from folo_source.core import Frame
from folo_source.inputs.base import InputKind, InputState

NOT_YET = 'Screen capture needs folo-capture (RB-14)'


class ScreenInput:
    """The screen `screen_id` (sources.json's screen inputs)."""

    def __init__(self, screen_id: str, enabled: bool = False) -> None:
        self.screen_id = screen_id
        self.enabled = enabled

    @property
    def kind(self) -> InputKind:
        return 'screen'

    @property
    def thread(self) -> threading.Thread | None:
        return None

    def start(self) -> None:
        pass

    def stop(self) -> None:
        pass

    def health(self) -> InputState:
        return 'error' if self.enabled else 'idle'

    def status(self) -> dict[str, Any]:
        return {'id': self.screen_id, 'enabled': self.enabled, 'message': NOT_YET}

    def latest(self) -> Frame | None:
        """The newest frame of the screen; none until the capture agent exists."""
        return None
