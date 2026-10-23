"""
The effect engine as its node runs it (RB-04 steps 7 and 8, ADR-0032): the
inputs, the executor on a render thread of its own, and the local link that
publishes to the node and takes its settings. The node's `config.apply`
starts the inputs; until then nothing is captured or rendered and only the
status heartbeat goes out.
"""

from __future__ import annotations

import base64
import logging
import threading
import time
from collections.abc import Callable, Mapping
from typing import Any, cast

from folo_source.contracts import clamp
from folo_source.core import now_us
from folo_source.inputs.audio.input import MAX_DT, AudioInput, AudioPacket
from folo_source.inputs.listing import list_inputs
from folo_source.inputs.media.input import MediaInput
from folo_source.publish import link as source_link
from folo_source.publish.link import LocalLink, RpcError
from folo_source.runtime import lifecycle
from folo_source.runtime.engine import IDLE_AFTER, IDLE_FPS
from folo_source.runtime.executor import Executor, Step

log = logging.getLogger(__name__)

# The engine must render at least 50 Hz (docs/architecture/pipeline.md#performance-budget)
BUDGET = 1.0 / 50.0
SLOW_WARN_EVERY = 10.0  # seconds between warnings about slow frames
MAX_PENDING = 50  # analysed packets kept for the next step; older ones are dropped
STATUS_EVERY = 1.0  # the heartbeat
ACTIVITY_EVERY = 0.1
INPUT_EVERY = 0.1
METRICS_EVERY = 1.0
# JSON-RPC: what the request asked for isn't there (no player, no such art)
NOT_AVAILABLE = -32000
MEDIA_ACTIONS = ('play_pause', 'next', 'previous')


class SourceEngine:
    """
    The engine on the local link at `url` with `token`. `on_end(reason)` is
    called once it should exit: the link gave up (`refused`, `lost`,
    `failed`) or the node asked it to restart (`restart`). `seed` makes the
    random modes repeatable.
    """

    def __init__(
        self, url: str, token: str, on_end: Callable[[str], None], seed: int | None = None
    ) -> None:
        self.on_end = on_end
        self._inputs: dict[str, Any] = cast(dict[str, Any], clamp.clamp('SourceInputs', {}))
        self.audio = AudioInput(lambda: self._inputs['audio'], self.submit)
        self.media = MediaInput(
            lambda: self._inputs['media']['enabled'], lambda: self._inputs['audio']
        )
        self.link = LocalLink(url, token, self.handle, on_end)
        self.executor = Executor(
            self.audio.analyzer, lifecycle.random_generator(seed), self.media, self.link.send_frame
        )
        self.state = 'starting'
        self.running = False
        self.thread: threading.Thread | None = None
        self._lock = threading.Lock()  # config.apply against rendering
        self._wake = threading.Condition()
        self._pending: list[AudioPacket] = []
        self._inputs_started = False
        self._sent: dict[str, float] = {}  # when each notification last went out
        self._last: dict[str, Any] = {}  # what it said, for the ones sent on change
        self._renders = 0
        self._render_s = 0.0
        self._skipped = 0
        self._published = 0
        self._link_dropped = 0
        self._slow_warned = 0.0

    def start(self) -> None:
        self.running = True
        self.thread = threading.Thread(target=self._run, name='render', daemon=True)
        self.thread.start()
        self.link.start()

    def stop(self, timeout: float = lifecycle.JOIN_TIMEOUT) -> None:
        """Stops every thread of the engine, waiting at most `timeout` seconds for them all."""
        self.state = 'stopping'
        self._send_status()
        self.running = False
        with self._wake:
            self._wake.notify()
        self.link.stop()
        self.audio.stop()
        self.media.stop()
        lifecycle.join(
            [self.thread, self.link.thread, self.audio.thread, self.media.thread], timeout
        )

    def submit(self, packet: AudioPacket) -> None:
        """A new analysed packet, from the capture thread."""
        with self._wake:
            self._pending.append(packet)
            if len(self._pending) > MAX_PENDING:
                self._skipped += len(self._pending) - MAX_PENDING
                del self._pending[:-MAX_PENDING]
            self._wake.notify()

    # ------------------------------------------------------------ the node's requests

    def handle(self, method: str, params: Mapping[str, Any]) -> Any:
        """Answers a request from the node (on the link's thread)."""
        if method == 'config.apply':
            return self._apply(params)
        if method == 'inputs.list':
            return list_inputs(self.media.available)
        if method == 'media.control':
            return self._media_control(params)
        if method == 'media.art':
            return self._media_art(params)
        if method == 'restart':
            if params.get('component') != 'engine':
                raise RpcError(source_link.INVALID_PARAMS, 'the engine only restarts itself')
            self.on_end('restart')
            return {}
        raise RpcError(source_link.METHOD_NOT_FOUND, f'the effect engine does not handle {method}')

    def _apply(self, params: Mapping[str, Any]) -> dict[str, Any]:
        try:
            clamped = cast(dict[str, Any], clamp.clamp('ConfigApplyParams', dict(params)))
        except clamp.StructuralError as error:
            raise RpcError(source_link.INVALID_PARAMS, str(error)) from error
        with self._lock:
            self._inputs = clamped['inputs']
            self.executor.apply(clamped)
        if not self._inputs_started:
            self._inputs_started = True
            self.audio.start()
            self.media.start()
        self.state = 'running'
        log.info('applied configuration %d', self.executor.revision)
        return {}

    def _media_control(self, params: Mapping[str, Any]) -> dict[str, Any]:
        action = params.get('action')
        if action not in MEDIA_ACTIONS:
            raise RpcError(source_link.INVALID_PARAMS, f'action must be one of {MEDIA_ACTIONS}')
        if not self.media.control(str(action)):
            raise RpcError(NOT_AVAILABLE, 'no player took it')
        return {}

    def _media_art(self, params: Mapping[str, Any]) -> dict[str, Any]:
        art = self.media.art
        if art is None or params.get('art_id') != self.media.status()['art']:
            raise RpcError(NOT_AVAILABLE, 'that album art is not shown any more')
        data, content_type = art
        return {'content_type': content_type, 'data': base64.b64encode(data).decode('ascii')}

    # ------------------------------------------------------------ rendering

    def _run(self) -> None:
        """
        Renders a step for every batch of packets, or silence after
        IDLE_AFTER without one, and sends the notifications that are due.
        Packets that came while the last frame rendered are one step: they
        all reach the beat and the activity, and only the newest renders,
        so a slow frame skips frames instead of queueing them.
        """
        last = time.perf_counter()
        while self.running:
            with self._wake:
                if not self._pending and self.running:
                    self._wake.wait(1.0 / IDLE_FPS)
                batch, self._pending = self._pending, []
                self._skipped += max(0, len(batch) - 1)
            now = time.perf_counter()
            steps = [Step(p.spectrum, p.frames, p.dt) for p in batch]
            if not steps and now - last >= IDLE_AFTER:
                steps = [Step.silence(min(MAX_DT, now - last))]
            if steps:
                self._render(steps)
                last = time.perf_counter()
            self._notify(time.perf_counter())

    def _render(self, steps: list[Step]) -> None:
        if not self.executor.configured:
            return
        started = time.perf_counter()
        with self._lock:
            sent = self.executor.publisher.sent
            stamp = now_us()
            self.executor.step(steps, stamp)
            self._published += self.executor.publisher.sent - sent
            levels = self.executor.levels_due(stamp)
        for params in levels:
            self.link.notify('levels', params)
        took = time.perf_counter() - started
        self._renders += 1
        self._render_s += took
        if took > BUDGET and started - self._slow_warned >= SLOW_WARN_EVERY:
            self._slow_warned = started
            log.warning('a frame took %.1f ms: %s', took * 1000.0, self._slowest())

    def _slowest(self) -> str:
        """The surface and effect that took longest in the last step."""
        worst = ('', '', 0.0)
        for index, timings in self.executor.timings.items():
            for effect_id, seconds in timings.items():
                if seconds > worst[2]:
                    worst = (str(index), effect_id, seconds)
        return f'surface {worst[0]}, {worst[1]} {worst[2] * 1000.0:.1f} ms'

    # ------------------------------------------------------------ notifications

    def _due(self, name: str, every: float, now: float) -> bool:
        if now - self._sent.get(name, -1e9) < every:
            return False
        self._sent[name] = now
        return True

    def _notify(self, now: float) -> None:
        status = self._status()
        if status != self._last.get('status') or self._due('status', STATUS_EVERY, now):
            self._last['status'] = status
            self._sent['status'] = now
            self.link.notify('status', status)
        if not self._inputs_started:
            return
        activity = self.executor.activity.params()
        changed = activity['active'] != self._last.get('active')
        if self.executor.configured and (changed or self._due('activity', ACTIVITY_EVERY, now)):
            self._last['active'] = activity['active']
            self.link.notify('activity', activity)
        if self._due('input', INPUT_EVERY, now):
            self.link.notify('input', self.audio.status())
        media = self.media.status()
        if media != self._last.get('media'):
            self._last['media'] = media
            self.link.notify('media', media)
        since = self._sent.get('metrics', now)
        if self._due('metrics', METRICS_EVERY, now):
            self.link.notify('metrics', self._metrics(now - since))

    def _status(self) -> dict[str, Any]:
        """The status notification (source-link.json's StatusParams)."""
        inputs: dict[str, str] = {}
        errors: list[str] = []
        if self._inputs_started:
            inputs = {'audio': self.audio.health(), 'media': self.media.health()}
            if inputs['audio'] == 'error':
                errors.append(self.audio.capture.message)
            if inputs['media'] == 'error' and self._inputs['media']['enabled']:
                errors.append(self.media.message)
        state = self.state
        if state == 'running' and inputs.get('audio') == 'error':
            state = 'degraded'
        workers = {'engine': 'running' if self.running else 'stopped'}
        return {'state': state, 'workers': workers, 'inputs': inputs, 'errors': errors[:32]}

    def _send_status(self) -> None:
        self.link.notify('status', self._status())

    def _metrics(self, seconds: float) -> dict[str, Any]:
        """The metrics notification over the last `seconds`, and starts counting afresh."""
        surfaces = max(1, len(self.executor.publisher.surfaces))
        link_dropped = self.link.dropped
        with self._wake:
            skipped, self._skipped = self._skipped, 0
        renders, self._renders = self._renders, 0
        render_s, self._render_s = self._render_s, 0.0
        published, self._published = self._published, 0
        dropped = skipped + link_dropped - self._link_dropped
        self._link_dropped = link_dropped
        return {
            'render_ms': round(1000.0 * render_s / renders, 3) if renders else 0.0,
            'publish_fps': round(published / max(seconds, 1e-3) / surfaces, 1),
            'dropped': max(0, dropped),
        }
