"""
What is playing, from Windows' media sessions: the media controls that
browsers and players report to (Windows 10 1809 and later). Gives the title,
artist, album, whether it plays, and the album art with its main colors,
picked for the leds (cover_colors).

Needs the winrt packages in requirements.txt. Without them `state` is
'unavailable' and everything else keeps working.
"""

import threading
import time

import numpy as np

from folo_source.inputs.media.art import ART_PIXELS, MAX_ART_BYTES, cover_colors

# Optional, and only on Windows: the type checker may not find them either
try:
    from winrt.windows.graphics.imaging import (  # pyright: ignore[reportMissingImports]
        BitmapAlphaMode,
        BitmapDecoder,
        BitmapInterpolationMode,
        BitmapPixelFormat,
        BitmapTransform,
        ColorManagementMode,
        ExifOrientationMode,
    )
    from winrt.windows.media.control import (  # pyright: ignore[reportMissingImports]
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    )
    from winrt.windows.media.control import (  # pyright: ignore[reportMissingImports]
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    )
    from winrt.windows.storage.streams import (  # pyright: ignore[reportMissingImports]
        Buffer,
        InputStreamOptions,
    )
except ImportError:  # pragma: no cover - the studio runs without it
    SessionManager = None

CHECK_EVERY = 1.0  # seconds between checks, besides the sessions' change events
PROPERTIES_EVERY = 5.0  # seconds between reading the title anyway (missed events)
ART_RETRY = 2.0  # players often set the art a moment after the title
# Short names for players whose app id says little
APP_NAMES = {
    'zunemusic': 'Media Player',
    'msedge': 'Edge',
    'chrome': 'Chrome',
    'firefox': 'Firefox',
    'spotify': 'Spotify',
    'spotifymusic': 'Spotify',
    'arc': 'Arc',
    'vlc': 'VLC',
    'applemusic': 'Apple Music',
    'opera': 'Opera',
    'brave': 'Brave',
    'tidal': 'TIDAL',
}
ACTIONS = ('play_pause', 'next', 'previous')


def app_label(app_id):
    """
    A readable name for a media session's app id ('Arc.exe',
    'SpotifyAB.SpotifyMusic_x!Spotify').
    """
    name = app_id.split('!')[-1]
    if name.lower().endswith('.exe'):
        name = name[:-4]
    name = name.split('.')[-1] or app_id
    return APP_NAMES.get(name.lower(), name)


class NowPlaying:
    """
    Runs a thread that follows Windows' media sessions while `enabled()`.
    Prefers a session that is playing, and among those the app the audio
    input captures (`audio()`: the audio input settings, `app` and
    `app_mode`). `track` counts the songs seen, so others can tell when a
    new one starts.
    """

    def __init__(self, enabled, audio):
        self.enabled = enabled
        self.audio = audio
        self.running = False
        self.thread: threading.Thread | None = None
        self._wake = threading.Event()
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
        self._manager = None
        self._session = None
        self._session_id = None
        self._tokens = []
        self._session_tokens = []
        self._properties_due = 0.0
        self._properties_dirty = True
        self._art_due = None
        self._key = None

    @property
    def paused(self) -> bool:
        """A session is open but nothing plays: paused or stopped."""
        return self.state in ('paused', 'stopped')

    def start(self):
        self.running = True
        self.thread = threading.Thread(target=self._loop, name='now-playing')
        self.thread.daemon = True
        self.thread.start()

    def stop(self):
        self.running = False
        self._wake.set()

    def status(self):
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
        """Play/pause, next or previous on the session shown. True if the player took it."""
        session = self._session
        if session is None or action not in ACTIONS:
            return False
        call = {
            'play_pause': session.try_toggle_play_pause_async,
            'next': session.try_skip_next_async,
            'previous': session.try_skip_previous_async,
        }[action]
        try:
            done = bool(call().get())
        except Exception:  # pylint: disable=broad-except
            return False
        self._wake.set()
        return done

    # -------------------------------------------------------------- thread

    def _loop(self):
        while self.running:
            if not self.enabled():
                self._release()
                self._clear('off')
            elif SessionManager is None:
                self._clear('unavailable', 'Install the winrt packages from requirements.txt')
            else:
                try:
                    if self._manager is None:
                        self._manager = SessionManager.request_async().get()
                        self._tokens = [
                            self._manager.add_current_session_changed(self._changed),
                            self._manager.add_sessions_changed(self._changed),
                        ]
                    self._refresh()
                except Exception as exc:  # pylint: disable=broad-except
                    self._release()
                    self._clear('error', str(exc))
            self._wake.wait(CHECK_EVERY)
            self._wake.clear()
        self._release()

    def _changed(self, sender, args):  # pylint: disable=unused-argument
        # WinRT calls these on its own threads: only flag and wake
        self._wake.set()

    def _properties_changed(self, sender, args):  # pylint: disable=unused-argument
        self._properties_dirty = True
        self._wake.set()

    def _release(self):
        """Unregisters the events and forgets the session manager."""
        self._watch(None)
        if self._manager is not None:
            try:
                self._manager.remove_current_session_changed(self._tokens[0])
                self._manager.remove_sessions_changed(self._tokens[1])
            except Exception:  # pylint: disable=broad-except
                pass
        self._manager = None
        self._tokens = []

    def _clear(self, state, message=''):
        self.state = state
        self.message = message
        self.app = self.title = self.artist = self.album = ''
        self.art = None
        self.colors = ()
        self._key = None

    def _pick(self):
        """The session to show: playing ones first, the captured app first among them."""
        manager = self._manager
        assert manager is not None, 'called with a session manager'
        sessions = list(manager.get_sessions())
        if not sessions:
            return None
        current = manager.get_current_session()
        audio = self.audio()
        wanted = audio['app'][:-4] if audio['app'].endswith('.exe') else audio['app']

        def rank(session):
            playing = session.get_playback_info().playback_status == PlaybackStatus.PLAYING
            app = (
                bool(wanted)
                and audio['app_mode'] == 'only'
                and wanted in session.source_app_user_model_id.lower()
            )
            is_current = (
                current is not None
                and session.source_app_user_model_id == current.source_app_user_model_id
            )
            return (not (playing and app), not playing, not app, not is_current)

        return min(sessions, key=rank)

    def _watch(self, session):
        """Follows one session's events (None to stop)."""
        if self._session is not None:
            try:
                self._session.remove_playback_info_changed(self._session_tokens[0])
                self._session.remove_media_properties_changed(self._session_tokens[1])
            except Exception:  # pylint: disable=broad-except
                pass
        self._session = session
        self._session_tokens = []
        self._session_id = session.source_app_user_model_id if session is not None else None
        if session is not None:
            self._session_tokens = [
                session.add_playback_info_changed(self._changed),
                session.add_media_properties_changed(self._properties_changed),
            ]
            self._properties_dirty = True

    def _refresh(self):
        session = self._pick()
        if session is None:
            self._watch(None)
            self._clear('none')
            return
        if session.source_app_user_model_id != self._session_id:
            self._watch(session)
        status = session.get_playback_info().playback_status
        self.state = {
            PlaybackStatus.PLAYING: 'playing',
            PlaybackStatus.PAUSED: 'paused',
            PlaybackStatus.STOPPED: 'stopped',
            PlaybackStatus.CLOSED: 'stopped',
        }.get(status, 'paused')
        self.message = ''
        self.app = app_label(session.source_app_user_model_id)

        now = time.monotonic()
        if not (
            self._properties_dirty
            or now >= self._properties_due
            or (self._art_due is not None and now >= self._art_due)
        ):
            return
        changed = self._properties_dirty
        self._properties_dirty = False
        self._properties_due = now + PROPERTIES_EVERY
        props = session.try_get_media_properties_async().get()
        if props is None:
            return
        key = (self._session_id, props.title or '', props.artist or '', props.album_title or '')
        new_song = key != self._key
        if new_song:
            self._key = key
            self.title, self.artist, self.album = key[1:]
            if key[1]:
                self.track += 1
            self.art = None
            self.colors = ()
            self._art_due = now
        elif changed and not self.art:
            self._art_due = now
        if self._art_due is not None and now >= self._art_due:
            self._read_art(props.thumbnail)
            self._art_due = None if self.art else (now + ART_RETRY if new_song else None)

    def _read_art(self, thumbnail):
        """Reads the album art and picks its colors."""
        if thumbnail is None:
            return
        try:
            stream = thumbnail.open_read_async().get()
            size = min(int(stream.size), MAX_ART_BYTES)
            if not size:
                return
            data = bytes(stream.read_async(Buffer(size), size, InputStreamOptions.READ_AHEAD).get())
            stream.seek(0)
            decoder = BitmapDecoder.create_async(stream).get()
            transform = BitmapTransform()
            transform.scaled_width = ART_PIXELS
            transform.scaled_height = ART_PIXELS
            transform.interpolation_mode = BitmapInterpolationMode.FANT
            pixels = decoder.get_pixel_data_transformed_async(
                BitmapPixelFormat.RGBA8,
                BitmapAlphaMode.IGNORE,
                transform,
                ExifOrientationMode.RESPECT_EXIF_ORIENTATION,
                ColorManagementMode.DO_NOT_COLOR_MANAGE,
            ).get()
            rgba = np.frombuffer(bytes(pixels.detach_pixel_data()), dtype=np.uint8)
        except Exception:  # pylint: disable=broad-except
            return
        self.colors = cover_colors(rgba.reshape(-1, 4)[:, :3])
        self.art = (data, stream.content_type or 'image/png')
        self.art_id += 1
