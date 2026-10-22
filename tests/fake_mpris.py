"""
A private session D-Bus and a fake MPRIS player on it, for testing now
playing on Linux without a desktop or a real player.
"""

# dbus-fast reads the D-Bus signatures from the annotations ('s', 'a{sv}'),
# which aren't Python types: pyright ignores this file (pyproject.toml)

import asyncio
import os
import shutil
import signal
import subprocess
import threading
from typing import Any

from dbus_fast import BusType, Variant
from dbus_fast.aio import MessageBus
from dbus_fast.constants import PropertyAccess
from dbus_fast.service import ServiceInterface, dbus_property, method

NAME = 'org.mpris.MediaPlayer2.folotest'
PATH = '/org/mpris/MediaPlayer2'
TIMEOUT = 5.0


class SessionBus:
    """A dbus-daemon of its own; `address` goes in DBUS_SESSION_BUS_ADDRESS."""

    def __init__(self) -> None:
        daemon = shutil.which('dbus-daemon')
        if daemon is None:
            raise RuntimeError('no dbus-daemon')
        out = subprocess.run(
            [daemon, '--session', '--fork', '--print-address=1', '--print-pid=1'],
            capture_output=True,
            text=True,
            check=True,
            timeout=TIMEOUT,
        ).stdout.split()
        self.address, self.pid = out[0], int(out[1])

    def stop(self) -> None:
        os.kill(self.pid, signal.SIGTERM)


class _Root(ServiceInterface):
    def __init__(self) -> None:
        super().__init__('org.mpris.MediaPlayer2')

    @dbus_property(access=PropertyAccess.READ)
    def Identity(self) -> 's':  # noqa: F821
        return 'FOLO Test Player'


class _Player(ServiceInterface):
    def __init__(self) -> None:
        super().__init__('org.mpris.MediaPlayer2.Player')
        self.status = 'Playing'
        self.metadata: dict[str, Variant] = {}
        self.calls: list[str] = []

    @dbus_property(access=PropertyAccess.READ)
    def PlaybackStatus(self) -> 's':  # noqa: F821
        return self.status

    @dbus_property(access=PropertyAccess.READ)
    def Metadata(self) -> 'a{sv}':  # noqa: F722
        return self.metadata

    @method()
    def PlayPause(self) -> None:
        self.calls.append('PlayPause')
        self.status = 'Paused' if self.status == 'Playing' else 'Playing'
        self.emit_properties_changed({'PlaybackStatus': self.status})

    @method()
    def Next(self) -> None:
        self.calls.append('Next')

    @method()
    def Previous(self) -> None:
        self.calls.append('Previous')


class FakePlayer:
    """An MPRIS player on the bus at `address`, changed from the test's thread."""

    def __init__(self, address: str) -> None:
        self.address = address
        self.player = _Player()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._bus: Any = None
        self._ready = threading.Event()
        self._stop: asyncio.Event | None = None
        self._thread = threading.Thread(target=self._run, name='fake-mpris', daemon=True)

    def start(self) -> 'FakePlayer':
        self._thread.start()
        assert self._ready.wait(TIMEOUT), 'the fake player did not start'
        return self

    def stop(self) -> None:
        if self._loop is not None and self._stop is not None:
            self._loop.call_soon_threadsafe(self._stop.set)
        self._thread.join(TIMEOUT)

    def play(self, title: str, artist: list[str], album: str, art_url: str = '') -> None:
        metadata = {
            'xesam:title': Variant('s', title),
            'xesam:artist': Variant('as', artist),
            'xesam:album': Variant('s', album),
        }
        if art_url:
            metadata['mpris:artUrl'] = Variant('s', art_url)
        self._change(lambda: self._set(metadata, 'Playing'))

    def pause(self) -> None:
        self._change(lambda: self._set(self.player.metadata, 'Paused'))

    def _set(self, metadata: dict[str, Variant], status: str) -> None:
        self.player.metadata, self.player.status = metadata, status
        self.player.emit_properties_changed({'Metadata': metadata, 'PlaybackStatus': status})

    def _change(self, change: Any) -> None:
        assert self._loop is not None
        done = threading.Event()
        self._loop.call_soon_threadsafe(lambda: (change(), done.set()))
        assert done.wait(TIMEOUT)

    def _run(self) -> None:
        asyncio.run(self._main())

    async def _main(self) -> None:
        self._loop = asyncio.get_running_loop()
        self._stop = asyncio.Event()
        self._bus = await MessageBus(bus_address=self.address, bus_type=BusType.SESSION).connect()
        self._bus.export(PATH, _Root())
        self._bus.export(PATH, self.player)
        await self._bus.request_name(NAME)
        self._ready.set()
        await self._stop.wait()
        self._bus.disconnect()
