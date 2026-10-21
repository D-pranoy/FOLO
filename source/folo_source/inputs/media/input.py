"""
The media input (RB-04 step 2): what's playing, its album art and colours,
and the player controls. Windows reads its media sessions (windows.py).
Linux reads MPRIS through dbus-fast, which isn't implemented yet; until then
the input reports itself unavailable there.
"""

from __future__ import annotations

import sys
import threading
from collections.abc import Callable, Mapping
from typing import Any

from folo_source.inputs.base import InputKind, InputState
from folo_source.inputs.media.windows import NowPlaying

NOT_YET = 'Now playing on Linux reads MPRIS, which is not implemented yet'

# The media states as the source's status notification names them
_HEALTH: dict[str, InputState] = {
    'playing': 'ok',
    'paused': 'ok',
    'stopped': 'ok',
    'none': 'ok',
    'error': 'error',
    'unavailable': 'error',
}


class MediaInput:
    """
    Follows the player while `enabled()` (the media input's `enabled`).
    `audio()` returns the audio input settings: the player of the app the
    audio input captures is preferred.
    """

    def __init__(self, enabled: Callable[[], bool], audio: Callable[[], Mapping[str, Any]]) -> None:
        self.session: NowPlaying | None = (
            NowPlaying(enabled, audio) if sys.platform == 'win32' else None
        )

    @property
    def kind(self) -> InputKind:
        return 'media'

    @property
    def thread(self) -> threading.Thread | None:
        return self.session.thread if self.session is not None else None

    @property
    def available(self) -> bool:
        """Whether now playing can be read on this source (the input list's `media`)."""
        return self.session is not None and self.session.state != 'unavailable'

    @property
    def track(self) -> int:
        """Counts the songs seen, so a new one can be told apart."""
        return self.session.track if self.session is not None else 0

    @property
    def paused(self) -> bool:
        return self.session.paused if self.session is not None else False

    @property
    def colors(self) -> tuple[str, ...]:
        """Up to five colours from the album art."""
        return tuple(self.session.colors) if self.session is not None else ()

    @property
    def art(self) -> tuple[bytes, str] | None:
        """The album art and its content type."""
        return self.session.art if self.session is not None else None

    def start(self) -> None:
        if self.session is not None:
            self.session.start()

    def stop(self) -> None:
        if self.session is not None:
            self.session.stop()

    def health(self) -> InputState:
        if self.session is None:
            return 'error'
        return _HEALTH.get(self.session.state, 'idle')

    @property
    def message(self) -> str:
        """Why it's unavailable or failed, if it is."""
        return self.session.message if self.session is not None else NOT_YET

    def status(self) -> dict[str, Any]:
        """Now playing, as the media notification sends it (api.json's MediaState)."""
        session = self.session
        if session is None:
            return {
                'state': 'unavailable',
                'app': '',
                'title': '',
                'artist': '',
                'album': '',
                'art': 0,
                'colors': [],
            }
        return {
            'state': session.state if session.state != 'starting' else 'none',
            'app': str(session.app or ''),
            'title': session.title,
            'artist': session.artist,
            'album': session.album,
            'art': session.art_id if session.art else 0,
            'colors': list(session.colors),
        }

    def control(self, action: str) -> bool:
        """play_pause, next or previous on the player shown. True if it took it."""
        return self.session.control(action) if self.session is not None else False
