"""
The engine's end of the local link (RB-04 step 7, ADR-0032), against a fake
node: the token handshake, frames and notifications with their type bytes,
answering requests, a refusal, reconnecting and giving up.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from folo_source.publish import frames, link
from folo_source.publish.link import LocalLink, RpcError
from scripts import conformance
from tests.fake_node import FakeNode

TOKEN = 'a' * 43


class Engine:
    """What the link calls back: answers requests and records why it ended."""

    def __init__(self) -> None:
        self.requests: list[tuple[str, Mapping[str, Any]]] = []
        self.ended = threading.Event()
        self.reason = ''

    def handle(self, method: str, params: Mapping[str, Any]) -> Any:
        self.requests.append((method, params))
        if method == 'config.apply':
            return {}
        if method == 'restart':
            raise ValueError('boom')
        raise RpcError(link.METHOD_NOT_FOUND, f'{method} is not implemented')

    def on_end(self, reason: str) -> None:
        self.reason = reason
        self.ended.set()


@pytest.fixture
def node() -> Iterator[FakeNode]:
    node = FakeNode(TOKEN).start()
    yield node
    node.stop()


def make_link(url: str, engine: Engine, token: str = TOKEN) -> LocalLink:
    linked = LocalLink(url, token, engine.handle, engine.on_end)
    linked.backoff_first = 0.05
    linked.backoff_max = 0.2
    return linked


@pytest.fixture
def engine() -> Engine:
    return Engine()


def stop(linked: LocalLink) -> None:
    linked.stop()
    assert linked.thread is not None
    linked.thread.join(3.0)
    assert not linked.thread.is_alive()


def test_settings_come_from_the_environment() -> None:
    assert link.link_settings({link.ENV_URL: 'ws://x', link.ENV_TOKEN: 't'}) == ('ws://x', 't')
    with pytest.raises(link.LinkError):
        link.link_settings({link.ENV_URL: 'ws://x'})


def test_handshake_and_messages(node: FakeNode, engine: Engine, tmp_path: Path) -> None:
    linked = make_link(node.url, engine)
    linked.start()
    try:
        assert linked.wait_connected(5.0)
        hello = node.hellos[0]
        assert hello['purpose'] == {'kind': 'local', 'token': TOKEN}
        assert link.CONTRACT_VERSION in hello['version']
        linked.send_frame(
            frames.surface_frame(2, np.zeros((4, 3), np.float32), 'rgb8', 0, 1, False)
        )
        linked.send_frame(frames.signal_frame(1, (120.0, 0.5, 1.0, 0.0, 0.0), 0, 1))
        status: dict[str, Any] = {
            'state': 'running',
            'workers': {'engine': 'running'},
            'inputs': {},
            'errors': [],
        }
        linked.notify('status', status)
        node.wait_for(lambda: node.notifications('status'))
        surface = node.wait_for(lambda: node.received(link.TYPE_SURFACE))[0]
        signal = node.wait_for(lambda: node.received(link.TYPE_SIGNAL))[0]
        assert conformance.check_frame(surface, 'surface') == []
        assert conformance.check_frame(signal, 'signal') == []
        # The messages conform to the contract
        (tmp_path / 'hello--engine.json').write_text(json.dumps(hello), encoding='utf-8')
        (tmp_path / 'notify.status--engine.json').write_text(
            json.dumps(node.notifications('status')[0]), encoding='utf-8'
        )
        assert conformance.check_messages(tmp_path) == []
    finally:
        stop(linked)


def test_requests_are_answered(node: FakeNode, engine: Engine, tmp_path: Path) -> None:
    linked = make_link(node.url, engine)
    linked.start()
    try:
        assert linked.wait_connected(5.0)
        ok = node.request('config.apply', {'revision': 1}, 1)
        assert ok == {'jsonrpc': '2.0', 'id': 1, 'result': {}}
        assert engine.requests[0] == ('config.apply', {'revision': 1})
        missing = node.request('pair.with', {'invite': 'x'}, 'two')
        assert missing['error']['code'] == link.METHOD_NOT_FOUND
        failed = node.request('restart', {'component': 'engine'}, 3)
        assert failed['error']['code'] == link.INTERNAL_ERROR
        node.send(bytes([link.TYPE_RPC]) + b'{not json')  # dropped, the link carries on
        bad = node.request('config.apply', [], 4)
        assert bad['error']['code'] == link.INVALID_PARAMS
        for name, payload in (('ok', ok), ('missing', missing), ('failed', failed)):
            (tmp_path / f'rpcResponse--{name}.json').write_text(
                json.dumps(payload), encoding='utf-8'
            )
        assert conformance.check_messages(tmp_path) == []
    finally:
        stop(linked)


def test_nothing_is_sent_before_the_handshake(engine: Engine) -> None:
    linked = make_link('ws://127.0.0.1:9/api/v1/source-link', engine)
    linked.notify('status', {})
    linked.send_frame(frames.signal_frame(0, (1.0,), 0, 0))
    assert linked.dropped == 0 and not linked.connected


def test_a_wrong_token_is_refused(node: FakeNode, engine: Engine) -> None:
    linked = make_link(node.url, engine, token='wrong')
    linked.start()
    assert engine.ended.wait(5.0)
    assert engine.reason == 'refused' == linked.ended
    assert linked.thread is not None
    linked.thread.join(3.0)
    assert not linked.thread.is_alive()


def test_it_reconnects_when_the_node_comes_back(engine: Engine) -> None:
    first = FakeNode(TOKEN).start()
    linked = make_link(first.url, engine)
    linked.start()
    try:
        assert linked.wait_connected(5.0)
        port = first.port
        first.stop()
        FakeNode.wait_for(lambda: not linked.connected)
        second = FakeNode(TOKEN, port=port).start()
        try:
            assert linked.wait_connected(5.0)
            assert second.connections == 1
            assert not engine.ended.is_set()
        finally:
            second.stop()
    finally:
        stop(linked)


def test_it_gives_up_without_a_node(engine: Engine) -> None:
    probe = FakeNode(TOKEN).start()
    url = probe.url
    probe.stop()  # nothing listens there now
    linked = make_link(url, engine)
    linked.give_up = 0.4
    linked.start()
    assert engine.ended.wait(5.0)
    assert engine.reason == 'lost'


def test_stop_ends_it_quickly(node: FakeNode, engine: Engine) -> None:
    linked = make_link(node.url, engine)
    linked.start()
    assert linked.wait_connected(5.0)
    stop(linked)
    assert not engine.ended.is_set()


def test_engine_summary_fits_the_contract() -> None:
    summary = link.engine_summary()
    assert set(summary) == {'id', 'name', 'version', 'platform'}
    assert all(0 < len(value) <= 32 for value in summary.values())
