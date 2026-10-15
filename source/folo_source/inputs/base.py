"""
What every input of a source has in common (RB-04 step 2): it starts and
stops its own work, if any, and reports its state. Inputs are listed and
enabled one by one (docs/architecture/pipeline.md#inputs-in-a-source).
"""

from __future__ import annotations

import threading
from collections.abc import Mapping
from typing import Any, Literal, Protocol

# What an input reports as its state (sources' `status` notification)
InputState = Literal['ok', 'waiting', 'error', 'idle']
InputKind = Literal['audio', 'media', 'generator', 'screen']


class Input(Protocol):
    """An input of the effect engine."""

    @property
    def kind(self) -> InputKind: ...

    @property
    def thread(self) -> threading.Thread | None:
        """The thread doing its work, if it has one; joined after stop()."""
        ...

    def start(self) -> None: ...

    def stop(self) -> None:
        """Asks its work to stop. The caller joins `thread`."""
        ...

    def health(self) -> InputState:
        """Its state as the source's status notification reports it."""
        ...

    def status(self) -> Mapping[str, Any]:
        """What it reports about itself, for notifications and the UI."""
        ...
