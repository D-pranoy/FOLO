"""
The effect engine as its node runs it (RB-04 steps 7 and 8, ADR-0032),
against a fake node: nothing but the heartbeat before `config.apply`, then
frames, signals and notifications that conform to the contract, idle
rendering, the requests it answers, and the `folo-source` command.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from folo_source.inputs.audio.analyzer import channel_peaks
from folo_source.publish import frames, link
from folo_source.runtime.source import NOT_AVAILABLE, SourceEngine
from scripts import conformance
from tests import golden_cases, synth
from tests.fake_node import FakeNode

TOKEN = 'b' * 43
ROOT = Path(__file__).resolve().parent.parent


def config_apply() -> dict[str, Any]:
    fixture = json.loads((conformance.FIXTURES / 'config.apply.json').read_text(encoding='utf-8'))
    return fixture['params']


@pytest.fixture
def node() -> Iterator[FakeNode]:
    node = FakeNode(TOKEN).start()
    yield node
    node.stop()


@pytest.fixture
def engine(
    node: FakeNode, monkeypatch: pytest.MonkeyPatch
) -> Iterator[tuple[SourceEngine, list[str]]]:
    ended: list[str] = []
    engine = SourceEngine(node.url, TOKEN, ended.append, seed=1)
    # No capture or players here: the test feeds the packets
    monkeypatch.setattr(engine.audio, 'start', lambda: None)
    monkeypatch.setattr(engine.media, 'start', lambda: None)
    engine.start()
    assert engine.link.wait_connected(5.0)
    yield engine, ended
    engine.stop()
    assert engine.thread is not None and not engine.thread.is_alive()


def feed(engine: SourceEngine, seconds: float) -> None:
    """Analysed packets of a synthetic song, as the capture thread would hand them over."""
    signal = golden_cases.signal('medley')
    for count, block in enumerate(synth.packets(signal)):
        if count * 0.01 >= seconds:
            break
        engine.submit(engine.audio.analyse(block, channel_peaks(block), False))
        threading.Event().wait(0.01)


def test_only_the_heartbeat_before_config_apply(
    node: FakeNode, engine: tuple[SourceEngine, list[str]]
) -> None:
    status = node.wait_for(lambda: node.notifications('status'))[0]
    assert status['params']['state'] == 'starting'
    threading.Event().wait(0.3)
    assert node.received(link.TYPE_SURFACE) == []
    assert node.notifications('activity') == []


def test_it_renders_and_publishes(
    node: FakeNode, engine: tuple[SourceEngine, list[str]], tmp_path: Path
) -> None:
    source, _ = engine
    assert node.request('config.apply', config_apply(), 1) == {
        'jsonrpc': '2.0',
        'id': 1,
        'result': {},
    }
    feed(source, 1.5)
    node.wait_for(lambda: node.notifications('metrics'))
    surfaces = [frames.decode_surface_frame(f) for f in node.received(link.TYPE_SURFACE)]
    assert len(surfaces) > 50
    assert all(f.pixels.shape == (1, 150, 3) and f.pixel_format == 'rgb8' for f in surfaces)
    sequences = [f.sequence for f in surfaces]
    assert sequences == sorted(sequences)
    assert any(f.pixels.any() for f in surfaces)  # the music lit something
    beat = [frames.decode_signal_frame(f) for f in node.received(link.TYPE_SIGNAL)]
    assert beat and all(f.index == 0 and len(f.values) == 5 for f in beat)
    running = node.notifications('status')[-1]['params']
    assert running['state'] in ('running', 'degraded')
    for method in ('status', 'activity', 'input', 'media', 'metrics'):
        message = node.notifications(method)[-1]
        (tmp_path / f'notify.{method}--engine.json').write_text(
            json.dumps(message), encoding='utf-8'
        )
    assert conformance.check_messages(tmp_path) == []
    assert any(n['params']['active'] for n in node.notifications('activity'))


def test_it_renders_while_no_audio_comes(
    node: FakeNode, engine: tuple[SourceEngine, list[str]]
) -> None:
    node.request('config.apply', config_apply(), 1)
    node.wait_for(lambda: len(node.received(link.TYPE_SURFACE)) >= 10)
    quiet = node.wait_for(
        lambda: [n for n in node.notifications('activity') if n['params']['quiet_s'] > 0.2]
    )
    assert quiet[-1]['params']['active'] is False


def test_the_heartbeat_keeps_coming(node: FakeNode, engine: tuple[SourceEngine, list[str]]) -> None:
    threading.Event().wait(2.2)
    assert len(node.notifications('status')) >= 2


def test_requests(node: FakeNode, engine: tuple[SourceEngine, list[str]]) -> None:
    _, ended = engine
    listing = node.request('inputs.list', {}, 1)['result']
    assert set(listing) >= {'outputs', 'inputs', 'apps', 'screens', 'media'}
    assert (
        node.request('media.control', {'action': 'louder'}, 2)['error']['code']
        == link.INVALID_PARAMS
    )
    assert node.request('media.art', {'art_id': 7}, 3)['error']['code'] == NOT_AVAILABLE
    assert node.request('packs.remove', {'id': 'x'}, 4)['error']['code'] == link.METHOD_NOT_FOUND
    assert node.request('restart', {'component': 'node'}, 5)['error']['code'] == link.INVALID_PARAMS
    assert node.request('restart', {'component': 'engine'}, 6)['result'] == {}
    assert ended == ['restart']


def test_config_apply_is_clamped(node: FakeNode, engine: tuple[SourceEngine, list[str]]) -> None:
    source, _ = engine
    params = config_apply()
    params['publish']['surfaces'][0]['rate_hz'] = 10_000
    params['surfaces'][0]['beat_offset_ms'] = 'soon'
    assert node.request('config.apply', params, 1)['result'] == {}
    assert source.executor.revision == 3


def run_command(env: dict[str, str]) -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        [sys.executable, '-m', 'folo_source', '--seed', '1'],
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_the_command_links_and_restarts(node: FakeNode) -> None:
    env = dict(os.environ, FOLO_SOURCE_LINK=node.url, FOLO_SOURCE_TOKEN=TOKEN)
    process = run_command(env)
    try:
        node.wait_for(lambda: node.notifications('status'), timeout=20.0)
        assert node.hellos[0]['purpose'] == {'kind': 'local', 'token': TOKEN}
        assert node.request('restart', {'component': 'engine'}, 1)['result'] == {}
        assert process.wait(10.0) == 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
    _, stderr = process.communicate()
    lines = [json.loads(line) for line in stderr.decode('utf-8').splitlines() if line.strip()]
    assert any(line['msg'] == 'stopped' for line in lines)
    assert TOKEN not in stderr.decode('utf-8')


def test_the_command_needs_its_link() -> None:
    env = {k: v for k, v in os.environ.items() if k not in (link.ENV_URL, link.ENV_TOKEN)}
    process = run_command(env)
    assert process.wait(20.0) == 2
    process.communicate()
