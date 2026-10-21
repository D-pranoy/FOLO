"""
Now playing on Linux (MPRIS, ADR-0028 D3) against a fake player on a
private session bus: the song and its player, album art from a local file
and its colours, following pauses as they happen, the player controls, and
no bus at all.

Runs on Linux with `dbus-daemon` installed. CI sets FOLO_DBUS_TESTS=1, which
turns a missing daemon into a failure instead of a skip.
"""

from __future__ import annotations

import os
import shutil
import sys
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from folo_source.contracts import models
from folo_source.inputs.media.input import MediaInput
from folo_source.inputs.media.linux import NowPlaying, read_art

pytestmark = pytest.mark.skipif(sys.platform != 'linux', reason='MPRIS is Linux')

TIMEOUT = 10.0


def wait(check: Any, timeout: float = TIMEOUT) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError('timed out')


@pytest.fixture
def bus(monkeypatch: pytest.MonkeyPatch) -> Iterator[str]:
    if shutil.which('dbus-daemon') is None:
        if os.environ.get('FOLO_DBUS_TESTS') == '1':
            pytest.fail('no dbus-daemon')
        pytest.skip('no dbus-daemon')
    from tests.fake_mpris import SessionBus

    session = SessionBus()
    monkeypatch.setenv('DBUS_SESSION_BUS_ADDRESS', session.address)
    yield session.address
    session.stop()


@pytest.fixture
def player(bus: str) -> Iterator[Any]:
    from tests.fake_mpris import FakePlayer

    fake = FakePlayer(bus).start()
    yield fake
    fake.stop()


@pytest.fixture
def playing() -> Iterator[tuple[NowPlaying, dict[str, bool]]]:
    switch = {'enabled': True}
    now = NowPlaying(lambda: switch['enabled'], lambda: {'app': '', 'app_mode': 'only'})
    now.start()
    yield now, switch
    now.stop()
    assert now.thread is not None
    now.thread.join(5.0)
    assert not now.thread.is_alive()


def magenta_art(folder: Path) -> Path:
    # Pillow is only installed on Linux, and pyright checks this on Windows too
    from PIL import Image  # pyright: ignore[reportMissingImports, reportUnknownVariableType]

    path = folder / 'cover art.png'  # a space, as file:// URLs quote it
    Image.new('RGB', (64, 64), (255, 0, 255)).save(path)  # pyright: ignore[reportUnknownMemberType]
    return path


def test_no_players(bus: str, playing: Any) -> None:
    now, _ = playing
    wait(lambda: now.state == 'none')


def test_the_song_and_its_player(player: Any, playing: Any, tmp_path: Path) -> None:
    now, _ = playing
    art = magenta_art(tmp_path)
    player.play('Song', ['Artist', 'Guest'], 'Album', art.as_uri())
    wait(lambda: now.title == 'Song' and now.art is not None)
    assert (now.state, now.app, now.artist, now.album) == (
        'playing',
        'FOLO Test Player',
        'Artist, Guest',
        'Album',
    )
    assert now.art is not None and now.art[1] == 'image/png'
    assert now.colors == ('#FF00FF',)
    first = now.track
    player.play('Another', ['Artist'], 'Album')
    wait(lambda: now.title == 'Another')
    assert now.track == first + 1 and now.art is None
    models.MediaState.model_validate({k: v for k, v in now.status().items() if k != 'message'})


def test_a_pause_shows_at_once(player: Any, playing: Any) -> None:
    now, _ = playing
    player.play('Song', ['Artist'], 'Album')
    wait(lambda: now.state == 'playing')
    started = time.monotonic()
    player.pause()
    wait(lambda: now.state == 'paused')
    assert now.paused
    assert time.monotonic() - started < 0.9  # the signal, not the next 1 s check


def test_controls(player: Any, playing: Any) -> None:
    now, _ = playing
    player.play('Song', ['Artist'], 'Album')
    wait(lambda: now.state == 'playing')
    assert now.control('next')
    assert now.control('play_pause')
    assert not now.control('louder')
    assert player.player.calls == ['Next', 'PlayPause']
    wait(lambda: now.state == 'paused')


def test_switched_off(player: Any, playing: Any) -> None:
    now, switch = playing
    player.play('Song', ['Artist'], 'Album')
    wait(lambda: now.state == 'playing')
    switch['enabled'] = False
    wait(lambda: now.state == 'off' and now.title == '')
    assert not now.control('next')


def test_no_session_bus(monkeypatch: pytest.MonkeyPatch, playing: Any) -> None:
    now, _ = playing
    monkeypatch.setenv('DBUS_SESSION_BUS_ADDRESS', 'unix:path=/nonexistent/folo-test')
    wait(lambda: now.state == 'unavailable')
    assert 'D-Bus' in now.message


def test_art_needs_a_local_image(tmp_path: Path) -> None:
    assert read_art('https://example.com/cover.jpg') is None
    assert read_art((tmp_path / 'missing.png').as_uri()) is None
    junk = tmp_path / 'junk.png'
    junk.write_bytes(b'not an image')
    assert read_art(junk.as_uri()) is None


def test_the_media_input_uses_mpris_on_linux() -> None:
    media = MediaInput(lambda: True, lambda: {'app': '', 'app_mode': 'only'})
    assert isinstance(media.session, NowPlaying)
