"""
A fake node for the effect engine's local link (ADR-0032): it serves the
source-link endpoint on loopback, checks the launch token, records what the
engine sends and can send it requests. The real node comes with RB-06/RB-07.
"""

from __future__ import annotations

import asyncio
import hmac
import json
import threading
import time
from collections.abc import Callable
from typing import Any

from websockets.asyncio.server import Server, ServerConnection, serve
from websockets.exceptions import ConnectionClosed

from folo_source.publish.link import SUBPROTOCOL, TYPE_RPC

TIMEOUT = 5.0


class FakeNode:
    """
    Serves ws://127.0.0.1:<port>/api/v1/source-link. `result` is the
    handshake answer for the right token; a wrong token is rejected.
    """

    def __init__(self, token: str, result: str = 'ok', port: int = 0) -> None:
        self.token = token
        self.result = result
        self.port = port
        self.hellos: list[dict[str, Any]] = []
        self.messages: list[bytes] = []
        self.connections = 0
        self._lock = threading.Lock()
        self._responses: dict[Any, dict[str, Any]] = {}
        self._loop: asyncio.AbstractEventLoop | None = None
        self._server: Server | None = None
        self._ws: ServerConnection | None = None
        self._ready = threading.Event()
        self._thread: threading.Thread | None = None

    @property
    def url(self) -> str:
        return f'ws://127.0.0.1:{self.port}/api/v1/source-link'

    def start(self) -> FakeNode:
        self._thread = threading.Thread(target=self._run, name='fake-node', daemon=True)
        self._thread.start()
        assert self._ready.wait(TIMEOUT), 'the fake node did not start'
        return self

    def stop(self) -> None:
        loop, server = self._loop, self._server
        if loop is not None and server is not None:
            loop.call_soon_threadsafe(server.close)
        if self._thread is not None:
            self._thread.join(TIMEOUT)

    def request(self, method: str, params: Any, rid: int | str) -> dict[str, Any]:
        """Sends a request to the linked engine and waits for its response."""
        self.send(
            bytes([TYPE_RPC])
            + json.dumps({'jsonrpc': '2.0', 'id': rid, 'method': method, 'params': params}).encode()
        )
        return self.wait_for(lambda: self._responses.get(rid))

    def send(self, message: bytes | str) -> None:
        loop, ws = self._loop, self._ws
        assert loop is not None and ws is not None, 'no engine linked'
        asyncio.run_coroutine_threadsafe(ws.send(message), loop).result(TIMEOUT)

    def received(self, kind: int) -> list[bytes]:
        """The messages of one type (their type byte removed)."""
        with self._lock:
            return [m[1:] for m in self.messages if m[:1] == bytes([kind])]

    def notifications(self, method: str) -> list[dict[str, Any]]:
        found: list[dict[str, Any]] = []
        for raw in self.received(TYPE_RPC):
            message = json.loads(raw)
            if message.get('method') == method and 'id' not in message:
                found.append(message)
        return found

    @staticmethod
    def wait_for[T](check: Callable[[], T | None], timeout: float = TIMEOUT) -> T:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = check()
            if value:
                return value
            time.sleep(0.01)
        raise AssertionError('timed out')

    def _run(self) -> None:
        asyncio.run(self._main())

    async def _main(self) -> None:
        self._loop = asyncio.get_running_loop()
        async with serve(self._serve, '127.0.0.1', self.port, subprotocols=[SUBPROTOCOL]) as server:
            self._server = server
            self.port = next(iter(server.sockets)).getsockname()[1]
            self._ready.set()
            await server.wait_closed()

    async def _serve(self, ws: ServerConnection) -> None:
        if ws.request is None or ws.request.path != '/api/v1/source-link':
            await ws.close()
            return
        hello = json.loads(await ws.recv())
        self.hellos.append(hello)
        token = str(hello.get('purpose', {}).get('token', ''))
        right = hmac.compare_digest(token.encode(), self.token.encode())
        result = self.result if right else 'rejected'
        if result == 'ok':  # before the engine can count itself linked
            self.connections += 1
            self._ws = ws
        await ws.send(json.dumps({'version': '1.2', 'result': result}))
        if result != 'ok':
            await ws.close()
            return
        try:
            async for message in ws:
                assert isinstance(message, bytes)
                with self._lock:
                    self.messages.append(message)
                if message[:1] == bytes([TYPE_RPC]):
                    body = json.loads(message[1:])
                    if 'id' in body and 'method' not in body:
                        self._responses[body['id']] = body
        except ConnectionClosed:
            pass
        finally:
            self._ws = None
