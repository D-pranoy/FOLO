"""
What is playing on Linux, from the players' MPRIS interfaces on the session
D-Bus (ADR-0028 D3), through dbus-fast: the title, artist, album, whether it
plays, the album art with its main colours (cover_colors), and the player
controls. The same interface as the Windows media sessions (windows.py).

Album art is read when the player names a local file (`file://`); art the
player only has on the web isn't fetched. Without a session bus `state` is
'unavailable' and everything else keeps working.
"""

from __future__ import annotations

import asyncio
import contextlib
import io
import threading
from collections.abc import Callable, Mapping
from typing import Any
from urllib.parse import unquote, urlparse

import numpy as np

from folo_source.inputs.media.art import ART_PIXELS, MAX_ART_BYTES, cover_colors

# Only on Linux: the type checker may not find them either
try:
    from dbus_fast import (  # pyright: ignore[reportMissingImports]
        BusType,
        Message,
        MessageType,
    )
    from dbus_fast.aio import MessageBus  # pyright: ignore[reportMissingImports]
    from PIL import Image  # pyright: ignore[reportMissingImports]
except ImportError:  # pragma: no cover - only on Linux
    MessageBus = None

CHECK_EVERY = 1.0  # seconds between checks, besides the players' change signals
CALL_TIMEOUT = 1.0  # seconds a player gets to answer
PREFIX = 'org.mpris.MediaPlayer2.'
PATH = '/org/mpris/MediaPlayer2'
ROOT = 'org.mpris.MediaPlayer2'
PLAYER = 'org.mpris.MediaPlayer2.Player'
PROPERTIES = 'org.freedesktop.DBus.Properties'
BUS = 'org.freedesktop.DBus'
BUS_PATH = '/org/freedesktop/DBus'
STATES = {'Playing': 'playing', 'Paused': 'paused', 'Stopped': 'stopped'}
METHODS = {'play_pause': 'PlayPause', 'next': 'Next', 'previous': 'Previous'}
MATCHES = (
    f"type='signal',interface='{PROPERTIES}',member='PropertiesChanged',path='{PATH}'",
    "type='signal',interface='org.freedesktop.DBus',member='NameOwnerChanged'",
)
NO_BUS = 'No session D-Bus: now playing needs the desktop session'


def _value(variant: Any) -> Any:
    """A D-Bus variant's value, or the value itself."""
    return getattr(variant, 'value', variant)


def _artist(value: Any) -> str:
    """xesam:artist is a list of names; some players send a string."""
    if isinstance(value, list | tuple):
        return ', '.join(str(v) for v in value)  # pyright: ignore[reportUnknownVariableType, reportUnknownArgumentType]
    return str(value or '')


def read_art(url: str) -> tuple[tuple[bytes, str], tuple[str, ...]] | None:
    """
    The album art at a file:// URL and its colours, or None when it isn't a
    local file or can't be read as an image.
    """
    parsed = urlparse(url)
    if parsed.scheme != 'file':
        return None
    try:
        with open(unquote(parsed.path), 'rb') as art:
            data = art.read(MAX_ART_BYTES + 1)
        if len(data) > MAX_ART_BYTES:
            return None
        image = Image.open(io.BytesIO(data))
        content_type = Image.MIME.get(image.format or '', 'image/png')
        small = image.convert('RGB').resize((ART_PIXELS, ART_PIXELS), Image.Resampling.BOX)
    except (OSError, ValueError):
        return None
    pixels = np.asarray(small, dtype=np.uint8)
    return (data, content_type), tuple(cover_colors(pixels))


class NowPlaying:
    """
    Runs a thread that follows the MPRIS players while `enabled()`, with a
    playing one first. `audio()` is the audio input's settings; Linux has
    no app capture to prefer a player by. `track` counts the songs seen.
    """

    def __init__(self, enabled: Callable[[], bool], audio: Callable[[], Mapping[str, Any]]) -> None:
        self.enabled = enabled
        self.audio = audio
        self.running = False
        self.thread: threading.Thread | None = None
        self.state: str = 'starting'  # off, unavailable, none, playing, paused, stopped, error
        self.message: str = ''
        self.app = ''
        self.title: str = ''
        self.artist: str = ''
        self.album: str = ''
        self.track: int = 0
        self.art: tuple[bytes, str] | None = None  # (bytes, content type)
        self.art_id: int = 0
        self.colors: tuple[str, ...] = ()
        self._player = ''  # the bus name of the player shown
        self._key: tuple[str, ...] | None = None
        self._art_url = ''
        self._identities: dict[str, str] = {}
        self._bus: Any = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._wake: asyncio.Event | None = None

    @property
    def paused(self) -> bool:
        """A player is there but nothing plays: paused or stopped."""
        return self.state in ('paused', 'stopped')

    def start(self) -> None:
        self.running = True
        self.thread = threading.Thread(target=self._run, name='now-playing', daemon=True)
        self.thread.start()

    def stop(self) -> None:
        self.running = False
        self._poke()

    def status(self) -> dict[str, Any]:
        return {
            'state': self.state,
            'message': self.message,
            'app': self.app,
            'title': self.title,
            'artist': self.artist,
            'album': self.album,
            'art': self.art_id if self.art else 0,
            'colors': list(self.colors),
        }

    def control(self, action: object) -> bool:
        """Play/pause, next or previous on the player shown. True if the player took it."""
        loop, player = self._loop, self._player
        if loop is None or not player or action not in METHODS:
            return False
        call = self._call(player, PLAYER, METHODS[str(action)])
        try:
            reply = asyncio.run_coroutine_threadsafe(call, loop).result(CALL_TIMEOUT + 0.5)
        except Exception:  # the player went away, or didn't answer
            return False
        self._poke()
        return reply.message_type == MessageType.METHOD_RETURN

    # -------------------------------------------------------------- thread

    def _poke(self) -> None:
        loop, wake = self._loop, self._wake
        if loop is not None and wake is not None:
            with contextlib.suppress(RuntimeError):  # its loop has finished
                loop.call_soon_threadsafe(wake.set)

    def _run(self) -> None:
        asyncio.run(self._main())

    async def _main(self) -> None:
        self._loop = asyncio.get_running_loop()
        self._wake = asyncio.Event()
        while self.running:
            if not self.enabled():
                self._disconnect()
                self._clear('off')
            elif MessageBus is None:
                self._clear('unavailable', 'Now playing on Linux needs dbus-fast and Pillow')
            else:
                await self._follow()
            with contextlib.suppress(TimeoutError):
                await asyncio.wait_for(self._wake.wait(), CHECK_EVERY)
            self._wake.clear()
        self._disconnect()

    async def _follow(self) -> None:
        """Connects when needed and reads the players again."""
        if self._bus is None:
            assert MessageBus is not None, 'checked by _main'
            try:
                self._bus = await asyncio.wait_for(
                    MessageBus(bus_type=BusType.SESSION).connect(), CALL_TIMEOUT
                )
            except Exception:
                self._bus = None
                self._clear('unavailable', NO_BUS)
                return
            self._bus.add_message_handler(self._on_message)
            for rule in MATCHES:
                await self._call(
                    BUS,
                    BUS,
                    'AddMatch',
                    's',
                    [rule],
                    BUS_PATH,
                )
        try:
            await self._refresh()
        except Exception as error:
            self._disconnect()
            self._clear('error', str(error) or type(error).__name__)

    def _on_message(self, message: Any) -> None:
        # A player changed, or one came or went: read them again now
        if message.member == 'PropertiesChanged' or (
            message.member == 'NameOwnerChanged' and str(message.body[0]).startswith(PREFIX)
        ):
            assert self._wake is not None
            self._wake.set()

    def _disconnect(self) -> None:
        if self._bus is not None:
            self._bus.disconnect()
        self._bus = None
        self._player = ''

    def _clear(self, state: str, message: str = '') -> None:
        self.state = state
        self.message = message
        self.app = self.title = self.artist = self.album = ''
        self.art = None
        self.colors = ()
        self._key = None
        self._art_url = ''

    async def _call(
        self,
        destination: str,
        interface: str,
        member: str,
        signature: str = '',
        body: list[Any] | None = None,
        path: str = PATH,
    ) -> Any:
        message = Message(
            destination=destination,
            path=path,
            interface=interface,
            member=member,
            signature=signature,
            body=body or [],
        )
        return await asyncio.wait_for(self._bus.call(message), CALL_TIMEOUT)

    async def _players(self) -> dict[str, dict[str, Any]]:
        """Every MPRIS player and its Player properties."""
        reply = await self._call(
            BUS,
            BUS,
            'ListNames',
            path=BUS_PATH,
        )
        names = sorted(str(n) for n in reply.body[0] if str(n).startswith(PREFIX))
        players: dict[str, dict[str, Any]] = {}
        for name in names:
            got = await self._call(name, PROPERTIES, 'GetAll', 's', [PLAYER])
            if got.message_type == MessageType.METHOD_RETURN:
                players[name] = {key: _value(value) for key, value in got.body[0].items()}
        return players

    async def _identity(self, name: str) -> str:
        """The player's own name for itself (MediaPlayer2.Identity)."""
        if name not in self._identities:
            got = await self._call(name, PROPERTIES, 'Get', 'ss', [ROOT, 'Identity'])
            ok = got.message_type == MessageType.METHOD_RETURN
            fallback = name.removeprefix(PREFIX).split('.')[0]
            self._identities[name] = str(_value(got.body[0])) if ok else fallback
        return self._identities[name]

    async def _refresh(self) -> None:
        players = await self._players()
        if not players:
            self._player = ''
            self._clear('none')
            return

        def rank(name: str) -> tuple[bool, bool, str]:
            playing = players[name].get('PlaybackStatus') == 'Playing'
            return (not playing, name != self._player, name)

        name = min(players, key=rank)
        props = players[name]
        self._player = name
        self.state = STATES.get(str(props.get('PlaybackStatus', '')), 'stopped')
        self.message = ''
        self.app = await self._identity(name)
        metadata = {key: _value(value) for key, value in dict(props.get('Metadata') or {}).items()}
        key = (
            name,
            str(metadata.get('xesam:title') or ''),
            _artist(metadata.get('xesam:artist')),
            str(metadata.get('xesam:album') or ''),
        )
        if key != self._key:
            self._key = key
            self.title, self.artist, self.album = key[1:]
            if key[1]:
                self.track += 1
            self.art, self.colors, self._art_url = None, (), ''
        self._update_art(str(metadata.get('mpris:artUrl') or ''))

    def _update_art(self, url: str) -> None:
        """Reads the art when the player names new art."""
        if url == self._art_url:
            return
        self._art_url = url
        found = read_art(url) if url else None
        if found is None:
            self.art, self.colors = None, ()
            return
        self.art, self.colors = found
        self.art_id += 1
