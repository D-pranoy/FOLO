"""
The screen input in the engine (RB-14 step 7, ADR-0037 D9, D11 and D12): a
surface's activity from its screen frames, with a fake clock; the render
trigger while audio is idle; a run against a fake node that sends `0x04`
frames and checks the published canvas; and the privacy markers, which no
status, level, log line, message or file may carry.
"""

from __future__ import annotations

import json
import logging
import threading
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from folo_source.inputs.audio.analyzer import Analyzer, channel_peaks
from folo_source.inputs.screen import STALE_US, ScreenInput
from folo_source.publish import frames, link
from folo_source.runtime.engine import IDLE_AFTER
from folo_source.runtime.executor import Executor, Step
from folo_source.runtime.source import SourceEngine
from scripts import conformance
from tests import golden_cases, synth
from tests import screen_frames as sf
from tests.fake_node import FakeNode

TOKEN = 'c' * 43
SECOND = 1_000_000


class Clock:
    def __init__(self) -> None:
        self.now = 10 * SECOND

    def __call__(self) -> int:
        return self.now


class Media:
    colors: tuple[str, ...] = ()
    track = 0


def audio_layer() -> dict[str, Any]:
    return {
        'id': 'a1',
        'effect': {'id': 'audio-visualizer'},
        'params': {'freq_lo': 30, 'freq_hi': 150, 'mode': 'fill'},
        'region': {'kind': 'relative', 'start': 0, 'end': 1},
    }


def make(
    layers: list[dict[str, Any]], clock: Clock, screens: bool = True
) -> tuple[Executor, ScreenInput, list[bytes]]:
    sent: list[bytes] = []
    screen = ScreenInput('', clock=clock)
    executor = Executor(Analyzer(), np.random.default_rng(1), Media(), sent.append, screen)
    executor.apply(sf.canvas_apply(layers, screens=screens))
    screen.configure('display-1', screens)
    return executor, screen, sent


# ---------------------------------------------------------------- activity


def test_a_screen_surface_has_its_own_activity() -> None:
    clock = Clock()
    executor, screen, _ = make([sf.screen_layer()], clock)
    assert executor.screen_wanted
    # No frame yet: inactive, counting from the first look
    assert executor.screen_activity(clock.now) == [{'surface': 0, 'active': False, 'quiet_s': 0.0}]
    clock.now += 2 * SECOND
    assert executor.screen_activity(clock.now)[0]['quiet_s'] == 2.0
    screen.push(sf.screen_frame(sf.solid(4, 4, (1, 2, 3))))
    assert executor.screen_activity(clock.now) == [{'surface': 0, 'active': True, 'quiet_s': 0.0}]
    clock.now += STALE_US - 1
    assert executor.screen_activity(clock.now)[0]['active'] is True
    clock.now += 1 + 250_000
    assert executor.screen_activity(clock.now) == [{'surface': 0, 'active': False, 'quiet_s': 0.25}]


def test_held_and_protected_frames_end_the_activity() -> None:
    clock = Clock()
    executor, screen, _ = make([sf.screen_layer()], clock)
    screen.push(sf.screen_frame(sf.solid(4, 4, (1, 2, 3))))
    for flag in (sf.HELD, sf.PROTECTED, sf.HELD):
        clock.now += 300_000
        screen.push(sf.screen_frame(sf.solid(4, 4, (1, 2, 3)), flags=flag))
    assert executor.screen_activity(clock.now) == [{'surface': 0, 'active': False, 'quiet_s': 0.4}]
    clock.now += 100_000
    screen.push(sf.screen_frame(sf.solid(4, 4, (1, 2, 3))))
    assert executor.screen_activity(clock.now)[0]['active'] is True


def test_a_surface_with_audio_layers_is_active_when_either_input_is() -> None:
    clock = Clock()
    executor, screen, _ = make([sf.screen_layer(), audio_layer()], clock)
    executor.activity.active, executor.activity.quiet_s = True, 0.0
    assert executor.screen_activity(clock.now)[0] == {'surface': 0, 'active': True, 'quiet_s': 0.0}
    # Audio falls silent while the screen is captured and unchanged
    screen.push(sf.screen_frame(sf.solid(4, 4, (1, 2, 3))))
    executor.activity.active, executor.activity.quiet_s = False, 5.0
    assert executor.screen_activity(clock.now)[0]['active'] is True
    # Both silent: the shorter silence counts
    clock.now += STALE_US + 1_000_000
    executor.activity.quiet_s = 5.0
    assert executor.screen_activity(clock.now)[0] == {'surface': 0, 'active': False, 'quiet_s': 1.0}
    executor.activity.quiet_s = 0.4
    assert executor.screen_activity(clock.now)[0]['quiet_s'] == 0.4


def test_other_surfaces_have_no_screen_activity() -> None:
    clock = Clock()
    executor, _, _ = make([audio_layer()], clock)
    assert not executor.screen_wanted and executor.screen_activity(clock.now) == []
    disabled = {**sf.screen_layer(), 'enabled': False}
    executor, _, _ = make([disabled], clock)
    assert not executor.screen_wanted and executor.screen_activity(clock.now) == []


def test_a_removed_screen_layer_forgets_where_its_silence_began() -> None:
    clock = Clock()
    executor, _, _ = make([sf.screen_layer()], clock)
    executor.screen_activity(clock.now)
    executor.apply(sf.canvas_apply([audio_layer()]))
    clock.now += 3600 * SECOND
    executor.apply(sf.canvas_apply([sf.screen_layer()]))
    assert executor.screen_activity(clock.now)[0]['quiet_s'] == 0.0


def test_the_executor_renders_the_picture_into_the_canvas() -> None:
    clock = Clock()
    executor, screen, sent = make([sf.screen_layer(smoothing=0)], clock)
    screen.push(sf.screen_frame(sf.solid(8, 16, (10, 20, 30))))
    executor.step([Step.silence(0.01)], 1000)
    decoded = frames.decode_surface_frame(sent[0])
    assert decoded.pixels.shape == (4, 8, 3)
    assert (decoded.pixels == [10, 20, 30]).all()


# ---------------------------------------------------------------- the engine


@pytest.fixture
def node() -> Iterator[FakeNode]:
    node = FakeNode(TOKEN).start()
    yield node
    node.stop()


@pytest.fixture
def engine(node: FakeNode, monkeypatch: pytest.MonkeyPatch) -> Iterator[SourceEngine]:
    engine = SourceEngine(node.url, TOKEN, lambda reason: None, seed=1)
    monkeypatch.setattr(engine.audio, 'start', lambda: None)
    monkeypatch.setattr(engine.media, 'start', lambda: None)
    engine.start()
    assert engine.link.wait_connected(5.0)
    yield engine
    engine.stop()


def near(pixels: Any, colour: tuple[int, int, int]) -> bool:
    """Whether every cell is `colour`, give or take the rounding of rgb8."""
    return bool((np.abs(pixels.astype(int) - np.array(colour)) <= 1).all())


def send_frame(node: FakeNode, data: bytes) -> None:
    node.send(bytes([link.TYPE_SCREEN]) + data)


def canvases(node: FakeNode) -> list[frames.SurfaceFrame]:
    return [frames.decode_surface_frame(f) for f in node.received(link.TYPE_SURFACE)]


def wait_for_canvas(node: FakeNode, colour: tuple[int, int, int]) -> frames.SurfaceFrame:
    return node.wait_for(
        lambda: next((c for c in reversed(canvases(node)) if near(c.pixels, colour)), None)
    )


def count_renders(engine: SourceEngine, monkeypatch: pytest.MonkeyPatch) -> list[float]:
    times: list[float] = []
    original = engine._render  # type: ignore[reportPrivateUsage]

    def counting(steps: list[Step]) -> None:
        times.append(time.perf_counter())
        original(steps)

    monkeypatch.setattr(engine, '_render', counting)
    return times


def test_it_greets_with_the_screen_frame_version(node: FakeNode, engine: SourceEngine) -> None:
    assert link.CONTRACT_VERSION == '1.4'
    assert node.hellos[0]['version'] == ['1.4', '1.3']


def test_screen_frames_become_a_canvas(node: FakeNode, engine: SourceEngine) -> None:
    layers = [sf.screen_layer(smoothing=0)]
    assert 'result' in node.request('config.apply', sf.canvas_apply(layers), 1)
    half = np.concatenate(
        [sf.solid(8, 8, (255, 0, 0)), sf.solid(8, 8, (0, 0, 255))], axis=1
    )  # red left, blue right
    send_frame(node, sf.screen_frame_bytes(half, sequence=1))
    node.wait_for(
        lambda: next(
            (
                c
                for c in canvases(node)
                if (c.pixels[:, :4] == [255, 0, 0]).all() and (c.pixels[:, 4:] == [0, 0, 255]).all()
            ),
            None,
        )
    )
    assert engine.screen.status()['state'] == 'ok'
    status = node.wait_for(lambda: node.notifications('status'))[-1]['params']
    assert status['inputs']['screen'] in ('ok', 'waiting')


def test_the_newest_frame_wins(node: FakeNode, engine: SourceEngine) -> None:
    node.request('config.apply', sf.canvas_apply([sf.screen_layer(smoothing=0)]), 1)
    for number, colour in enumerate([(200, 0, 0), (0, 200, 0), (0, 0, 200)]):
        send_frame(node, sf.screen_frame_bytes(sf.solid(8, 16, colour), sequence=number))
    wait_for_canvas(node, (0, 0, 200))
    threading.Event().wait(0.2)
    assert near(canvases(node)[-1].pixels, (0, 0, 200))


def test_a_malformed_frame_is_dropped_and_the_engine_carries_on(
    node: FakeNode, engine: SourceEngine, caplog: pytest.LogCaptureFixture
) -> None:
    caplog.set_level(logging.DEBUG)
    node.request('config.apply', sf.canvas_apply([sf.screen_layer(smoothing=0)]), 1)
    good = sf.screen_frame_bytes(sf.solid(8, 16, (50, 60, 70)))
    send_frame(node, good[:-1])  # one byte short
    send_frame(node, b'junk')
    send_frame(node, sf.with_header(good, 5, '<B', 3))
    node.wait_for(lambda: engine.link.screen_rejected >= 3)
    assert engine.screen.picture() is None
    send_frame(node, good)
    wait_for_canvas(node, (50, 60, 70))
    assert any('dropped a screen frame' in r.getMessage() for r in caplog.records)
    assert engine.link.connected


def test_bad_frame_warnings_are_rate_limited(
    node: FakeNode, engine: SourceEngine, caplog: pytest.LogCaptureFixture
) -> None:
    caplog.set_level(logging.WARNING)
    for _ in range(20):
        send_frame(node, b'junk')
    node.wait_for(lambda: engine.link.screen_rejected >= 20)
    warnings = [r for r in caplog.records if 'dropped a screen frame' in r.getMessage()]
    assert len(warnings) == 1


def test_the_surfaces_activity_is_notified(
    node: FakeNode, engine: SourceEngine, tmp_path: Path
) -> None:
    node.request('config.apply', sf.canvas_apply([sf.screen_layer(smoothing=0)]), 1)
    send_frame(node, sf.screen_frame_bytes(sf.solid(8, 16, (50, 60, 70))))

    def surface_activity() -> list[dict[str, Any]]:
        return [n for n in node.notifications('activity') if 'surface' in n['params']]

    active = node.wait_for(lambda: [n for n in surface_activity() if n['params']['active']])
    assert set(active[0]['params']) == {'surface', 'active', 'quiet_s'}  # no level_db
    assert active[0]['params']['surface'] == 0
    (tmp_path / 'notify.activity--screen.json').write_text(json.dumps(active[0]), encoding='utf-8')
    assert conformance.check_messages(tmp_path) == []
    # No more frames: it goes inactive and counts the silence
    quiet = node.wait_for(
        lambda: [n for n in surface_activity() if not n['params']['active']], timeout=3.0
    )
    assert quiet[-1]['params']['surface'] == 0


def test_a_screen_frame_starts_a_step_while_audio_is_idle(
    node: FakeNode, engine: SourceEngine, monkeypatch: pytest.MonkeyPatch
) -> None:
    node.request('config.apply', sf.canvas_apply([sf.screen_layer()]), 1)
    renders = count_renders(engine, monkeypatch)
    threading.Event().wait(0.5)
    idle = len(renders)
    del renders[:]
    for _ in range(50):  # 100 frames a second for half a second
        engine.screen.push(sf.screen_frame(sf.solid(8, 16, (50, 60, 70))))
        threading.Event().wait(0.01)
    assert len(renders) >= 3 * max(idle, 1), (len(renders), idle)
    assert len(renders) >= 25


def test_a_screen_frame_alone_starts_no_step_without_screen_layers(
    node: FakeNode, engine: SourceEngine, monkeypatch: pytest.MonkeyPatch
) -> None:
    node.request('config.apply', sf.canvas_apply([audio_layer()]), 1)
    renders = count_renders(engine, monkeypatch)
    for _ in range(30):
        engine.screen.push(sf.screen_frame(sf.solid(8, 16, (50, 60, 70))))
        threading.Event().wait(0.01)
    assert len(renders) <= 6  # only the idle steps


def test_audio_steps_carry_the_newest_frame_while_audio_plays(
    node: FakeNode, engine: SourceEngine, monkeypatch: pytest.MonkeyPatch
) -> None:
    node.request('config.apply', sf.canvas_apply([sf.screen_layer(smoothing=0)]), 1)
    signal = golden_cases.signal('medley')
    block = next(iter(synth.packets(signal)))
    threading.Event().wait(0.3)  # let the idle steps settle
    packet = engine.audio.analyse(block, channel_peaks(block), False)
    # When each step rendered, and whether it was the packet's
    renders: list[tuple[float, bool]] = []
    original = engine._render  # type: ignore[reportPrivateUsage]

    def recording(steps: list[Step]) -> None:
        renders.append((time.perf_counter(), any(s.spectrum is packet.spectrum for s in steps)))
        original(steps)

    monkeypatch.setattr(engine, '_render', recording)
    engine.submit(packet)
    started = time.perf_counter()
    while time.perf_counter() - started < 0.06:
        engine.screen.push(sf.screen_frame(sf.solid(8, 16, (50, 60, 70))))
        threading.Event().wait(0.005)
    deadline = time.perf_counter() + 2.0
    while not any(mine for _, mine in renders) and time.perf_counter() < deadline:
        threading.Event().wait(0.01)
    # The packet's step, and no step of the frames' own while audio is recent. An idle step
    # that fell due before the packet came may render first: it isn't counted.
    assert [mine for _, mine in renders].count(True) == 1
    audio_at = next(at for at, mine in renders if mine)
    assert [at for at, _ in renders if audio_at < at < audio_at + IDLE_AFTER] == []
    wait_for_canvas(node, (50, 60, 70))


# ---------------------------------------------------------------- privacy

INTERIOR = (40, 80, 120)
BARS = (254, 1, 253)  # in the black bars
MARGIN = (253, 2, 251)  # in the margins
HELD = (252, 3, 249)  # in a held frame
PROTECTED = (251, 4, 247)  # in a protected frame
MARKERS = {'bars': BARS, 'margin': MARGIN, 'held': HELD, 'protected': PROTECTED}


def marker_frames() -> tuple[bytes, bytes, bytes]:
    """A 12 x 12 picture: one cell of bars, two of margin (a fifth of 10), then the colour."""
    grid = sf.solid(12, 12, BARS)
    grid[1:11, 1:11] = MARGIN
    grid[3:9, 3:9] = INTERIOR
    shown = sf.screen_frame_bytes(grid, bars=(1, 1, 1, 1), sequence=1)
    held = sf.screen_frame_bytes(sf.solid(12, 12, HELD), flags=sf.HELD, sequence=2)
    protected = sf.screen_frame_bytes(sf.solid(12, 12, PROTECTED), flags=sf.PROTECTED, sequence=3)
    return shown, held, protected


def files_in(folder: Path) -> Iterator[bytes]:
    for path in folder.rglob('*'):
        if path.is_file():
            yield path.read_bytes()


def test_no_raw_pixel_leaves_the_engine(
    node: FakeNode,
    engine: SourceEngine,
    caplog: pytest.LogCaptureFixture,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = tmp_path / 'home'
    home.mkdir()
    monkeypatch.chdir(home)
    monkeypatch.setenv('TMPDIR', str(home))
    monkeypatch.setenv('HOME', str(home))
    caplog.set_level(logging.DEBUG)
    layers = [
        sf.screen_layer(smoothing=0, margins={'top': 0.2, 'bottom': 0.2, 'left': 0.2, 'right': 0.2})
    ]
    publish = {
        'surfaces': [
            {
                'index': 0,
                'width': 8,
                'height': 4,
                'rate_hz': 200,
                'pixel_format': 'rgb8',
                'lz4': False,
            }
        ],
        'signals': [{'name': 'spectrum', 'index': 0}, {'name': 'activity', 'index': 1}],
        'levels': [0],
    }
    node.request('config.apply', sf.canvas_apply(layers, publish=publish), 1)
    shown, held, protected = marker_frames()
    send_frame(node, shown)
    wait_for_canvas(node, INTERIOR)
    send_frame(node, held)
    send_frame(node, protected)
    send_frame(node, held[:-5])  # malformed: the log must say why, not what
    node.wait_for(lambda: engine.link.screen_rejected >= 1)
    engine.screen.push(sf.screen_frame(sf.solid(12, 12, HELD), flags=sf.HELD))
    threading.Event().wait(0.6)
    engine.stop()  # its last messages go out

    payloads = {
        name: sf.screen_frame(sf.solid(12, 12, marker)).pixels.tobytes()[:3]
        for name, marker in MARKERS.items()
    }
    raw = {shown[36:], held[36:], protected[36:]}
    surfaces = node.received(link.TYPE_SURFACE)
    others = [m for m in node.messages if m[:1] != bytes([link.TYPE_SURFACE])]
    assert surfaces and others
    # Everything but the canvas: not a marker, not a payload
    for message in others:
        for name, marker in payloads.items():
            assert marker not in message, name
        for payload in raw:
            assert payload not in message
    # The canvas is exactly the picture, the interior's colour, and no marker
    decoded = [frames.decode_surface_frame(m) for m in surfaces]
    shown_canvases = [d for d in decoded if near(d.pixels, INTERIOR)]
    assert shown_canvases
    for canvas in decoded:
        for marker in MARKERS.values():
            assert not (np.abs(canvas.pixels.astype(int) - marker) <= 1).all(axis=-1).any()
        assert (np.abs(canvas.pixels.astype(int) - INTERIOR) <= 1).all() or not canvas.pixels.any()
    # The log, the status and the input's own status
    text = caplog.text
    for marker in MARKERS.values():
        for form in (
            str(marker),
            ', '.join(map(str, marker)),
            bytes(marker).hex(),
            repr(bytes(marker)),
        ):
            assert form not in text, form
    for payload in raw:
        assert repr(payload)[:20] not in text
    status = json.dumps(engine._status()) + json.dumps(engine.screen.status())  # type: ignore[reportPrivateUsage]
    for marker in MARKERS.values():
        assert ', '.join(map(str, marker)) not in status
    # Nothing written to the folders the engine could use
    for content in files_in(home):
        for marker in payloads.values():
            assert marker not in content
