"""
The engine of today's studio: captures what is playing, analyses the
spectrum, tracks the beat, renders the configured segments to LED colors
and sends them to Hyperion. Renders silence while no audio arrives.
"""

import threading
import time

import numpy as np

from folo_source.inputs.audio.analyzer import SAMPLERATE, Spectrum
from folo_source.inputs.audio.input import MAX_DT, AudioInput, AudioPacket
from folo_source.inputs.audio.windows import audio_input
from folo_source.inputs.media import windows as now_playing
from folo_source.legacy.hyperion import HyperionSender
from folo_source.legacy.renderer import Renderer, shown_levels
from folo_source.runtime import lifecycle
from folo_source.signals import beat as beat_tracker

# The beat tracker runs while a segment uses it, and this many seconds after
# a page last streamed (they show the tempo)
BEAT_WATCH = 3.0
# Without audio packets (nothing playing on some devices, or no device) the
# engine still renders, so the ambient light fades in and keeps going
IDLE_AFTER = 0.1  # seconds without a packet before idle rendering starts
IDLE_FPS = 30


def audio_sources():
    """
    The default output, the devices and the apps playing audio (see
    audio_input.sources).
    """
    return audio_input.sources()


class StudioEngine:
    """
    Analyses the audio from the input thread (AudioInput) and
    keeps the latest spectrum, led frame and segment levels for the web UI
    and the hyperion sender. `store` holds the settings (its `config`), and
    `seed` makes the random modes repeatable.
    """

    def __init__(self, store, seed: int | None = None):
        self.store = store
        self.input = AudioInput(lambda: store.config['audio'], self._on_packet)
        self.analyzer = self.input.analyzer
        self.renderer = Renderer(self.analyzer, rng=lifecycle.random_generator(seed))
        self.level = self.input.level
        self.beat = beat_tracker.BeatTracker(self.analyzer.freqs, SAMPLERATE)
        self._beat_configs = (None, False)  # (config, whether a segment uses the beat)
        self._watched = 0.0  # when a page last streamed
        self.media = now_playing.NowPlaying(
            lambda: store.config['now_playing']['enabled'], lambda: store.config['audio']
        )
        self._track = 0  # the song the beat tracker has been following
        self.sender = HyperionSender(self)
        self.lock = threading.Lock()
        # The capture thread and the idle thread both render
        self._render_lock = threading.Lock()
        self._last_render = time.perf_counter()
        self.spectrum = Spectrum.silent()
        self.sequence = 0  # counts analysed packets
        self._display = (-1, [], {})  # what the UI shows of one packet: (sequence, bands, levels)
        self._display_plan = self.analyzer.band_plan(self.analyzer.display_edges)
        self.frame: bytes = b''
        self.levels = {}
        self.fps = 0.0
        self.running = False
        self._idle: threading.Thread | None = None
        self._frames = 0
        self._fps_time = time.perf_counter()

    def start(self):
        self.running = True
        self.media.start()
        self.input.start()
        self._idle = threading.Thread(target=self._idle_loop, name='idle-render', daemon=True)
        self._idle.start()
        self.sender.start()

    def stop(self, timeout: float = lifecycle.JOIN_TIMEOUT):
        """Stops every thread of the engine, waiting at most `timeout` seconds for them all."""
        self.running = False
        self.input.stop()
        self.media.stop()
        self.sender.stop()
        lifecycle.join(
            [self.input.thread, self.media.thread, self.sender.thread, self._idle], timeout
        )

    def _idle_loop(self):
        """
        Renders silence while no audio packets arrive, so time keeps moving
        for the ambient light, the release of the segments and the moving
        modes. Packets stop when nothing plays on some devices, or when the
        capture fails.
        """
        while self.running:
            time.sleep(1.0 / IDLE_FPS)
            gap = time.perf_counter() - self._last_render
            if gap >= IDLE_AFTER:
                with self._render_lock:
                    dt = min(MAX_DT, gap)
                    silence = Spectrum.silent()
                    self._track_beat(silence, int(dt * SAMPLERATE))
                    self._render(silence, dt, packet=False)

    def _beat_wanted(self, config):
        """Whether to track the beat: a segment uses it, or a page shows the tempo."""
        if self._beat_configs[0] is not config:
            used = any(s['trigger'] == 'beat' and s['enabled'] for s in config['segments'])
            self._beat_configs = (config, used)
        return self._beat_configs[1] or time.perf_counter() - self._watched < BEAT_WATCH

    def _track_beat(self, spectrum, frames):
        # Called with _render_lock held, before rendering the same audio
        config = self.store.config
        if not self._beat_wanted(config):
            self.beat.skip()
            return
        if self.media.track != self._track:
            # A new song: find its tempo afresh
            self._track = self.media.track
            self.beat.reset()
        self.beat.lead = beat_tracker.ONSET_DELAY + config['beat']['offset'] / 1000.0
        self.beat.push(spectrum, frames)

    def _render(self, spectrum, dt, packet):
        # Called with _render_lock held. fps counts audio packets only.
        config = self.store.config
        media = self.media
        frame, levels = self.renderer.render(
            config,
            spectrum,
            dt,
            beat=self.beat,
            colors=media.colors if config['now_playing']['cover_colors'] else (),
            paused=media.paused,
        )
        now = self._last_render = time.perf_counter()
        with self.lock:
            self.spectrum = spectrum
            self.frame = frame
            self.levels = levels
            self.sequence += 1
        self._frames += int(packet)
        if now - self._fps_time >= 1.0:
            self.fps = self._frames / (now - self._fps_time)
            self._frames = 0
            self._fps_time = now

    def _on_packet(self, packet: AudioPacket) -> None:
        """A new analysed packet from the input thread."""
        with self._render_lock:
            self._track_beat(packet.spectrum, packet.frames)
            self._render(packet.spectrum, packet.dt, packet=True)

    def snapshot(self):
        """
        Latest state for the web UI.
        """
        self._watched = time.perf_counter()
        with self.lock:
            spectrum, frame, levels, sequence = (
                self.spectrum,
                self.frame,
                self.levels,
                self.sequence,
            )
        # Every open page streams the same display bands and levels: work them
        # out once per frame, and only for frames that are streamed
        display = self._display
        if display[0] != sequence:
            bands = self.analyzer.planned_levels(spectrum, self._display_plan)
            display = self._display = (
                sequence,
                np.round(bands).astype(int).tolist(),
                shown_levels(levels),
            )
        return {
            'led_count': self.store.config['led_count'],
            'revision': self.store.revision,
            'server_id': self.store.server_id,
            'spectrum': display[1],
            'leds': frame.hex(),
            'levels': display[2],
            'status': {
                'audio': {
                    'ok': self.input.capture.state == 'ok',
                    'device': self.input.capture.label,
                    'message': self.input.capture.message,
                },
                'input': dict(
                    self.input.capture.status(),
                    gain=self.store.config['audio']['gain'],
                    auto=round(self.level.gain, 1),
                ),
                'hyperion': self.sender.status,
                'fps': round(self.fps),
                # 0 reactive ... 1 ambient light, and seconds of silence so far
                'ambient': round(self.renderer.ambient.amount, 3),
                'quiet': round(self.renderer.ambient.quiet, 1),
                'beat': self.beat_status(),
                'media': self.media.status(),
            },
        }

    def beat_status(self):
        beat = self.beat
        return {
            'bpm': round(beat.bpm, 1),
            'confidence': round(beat.confidence, 2),
            'locked': beat.locked,
            # share of the current beat gone by, and which beat of the bar it is (0 first)
            'phase': round(beat.phase, 2),
            'bar_beat': beat.bar_beat,
        }
