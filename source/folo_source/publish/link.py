"""
The local link (RB-04 step 7, docs/specs/source-link.md#the-local-link,
ADR-0032): the effect engine dials its node over loopback, proves it is the
node's own engine with the launch token, then sends surface frames, signal
frames and notifications, newest wins, and answers the node's JSON-RPC
requests. It runs on a thread of its own with an asyncio loop.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import platform
import random
import struct
import sys
import threading
import time
from collections import deque
from collections.abc import Callable, Mapping
from importlib import metadata
from typing import Any, Literal, cast

from websockets.asyncio.client import ClientConnection, connect
from websockets.exceptions import ConnectionClosed, WebSocketException
from websockets.typing import Subprotocol

from folo_source.publish.frames import SIGNAL_MAGIC, SURFACE_MAGIC

log = logging.getLogger(__name__)

SUBPROTOCOL = Subprotocol('folo.source-link.v1')
CONTRACT_VERSION = '1.3'
ENV_URL = 'FOLO_SOURCE_LINK'
ENV_TOKEN = 'FOLO_SOURCE_TOKEN'

# Message type bytes (docs/specs/source-link.md#messages-inside-the-noise-channel)
TYPE_RPC = 0x01
TYPE_SURFACE = 0x02
TYPE_SIGNAL = 0x03

# JSON-RPC 2.0 error codes
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603

BACKOFF_FIRST = 1.0  # seconds before the first reconnect
BACKOFF_MAX = 60.0
JITTER = 0.25  # each delay is cut by up to this share, at random
GIVE_UP = 30.0  # seconds without the node before the engine exits
OPEN_TIMEOUT = 5.0  # seconds for the connection and for the node's handshake response
CLOSE_TIMEOUT = 1.0
MAX_MESSAGE = 16 * 1024 * 1024  # bytes the node may send at once (config.apply, packs)

# What the node's requests are handed to: the method and its params, giving
# the result; RpcError for a JSON-RPC error
Handler = Callable[[str, Mapping[str, Any]], Any]
SlotKey = tuple[Literal['surface', 'signal', 'notify'], int | str]


class LinkError(Exception):
    """The link couldn't be set up; it is tried again."""


class Refused(LinkError):
    """The node answered the handshake with something other than `ok`."""


class RpcError(Exception):
    """A JSON-RPC error answering a request."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def link_settings(environ: Mapping[str, str]) -> tuple[str, str]:
    """The link's URL and launch token from the environment the node set."""
    url, token = environ.get(ENV_URL, ''), environ.get(ENV_TOKEN, '')
    if not url or not token:
        raise LinkError(f'{ENV_URL} and {ENV_TOKEN} must both be set by the node')
    return url, token


def engine_summary() -> dict[str, str]:
    """The engine as its handshake describes it (api.json's NodeSummary)."""
    try:
        version = metadata.version('folo-source').split('+')[0]
    except metadata.PackageNotFoundError:
        version = '0.0.0'
    system = {'win32': 'windows', 'darwin': 'macos'}.get(sys.platform, sys.platform)
    machine = platform.machine().lower()
    arch = {'amd64': 'x64', 'x86_64': 'x64', 'aarch64': 'arm64'}.get(machine, machine)
    return {
        'id': 'folo-source',
        'name': 'folo-source',
        'version': version,
        'platform': f'{system}-{arch}',
    }


def rpc_message(payload: Mapping[str, Any]) -> bytes:
    """A JSON-RPC message with its type byte."""
    return bytes([TYPE_RPC]) + json.dumps(payload, separators=(',', ':')).encode('utf-8')


def _frame_key(frame: bytes) -> tuple[SlotKey, int]:
    """A frame's newest-wins slot (its surface or signal index) and its type byte."""
    magic = frame[:4]
    if magic == SURFACE_MAGIC:
        return ('surface', struct.unpack_from('<H', frame, 8)[0]), TYPE_SURFACE
    if magic == SIGNAL_MAGIC:
        return ('signal', struct.unpack_from('<H', frame, 6)[0]), TYPE_SIGNAL
    raise ValueError('not a surface or signal frame')


class LocalLink:
    """
    The engine's end of the local link to `url` with `token`. `handler`
    answers the node's requests, on the link's thread, so it must be quick.
    `on_end(reason)` is called when the link gives up for good: the node
    refused it, or was gone for GIVE_UP seconds. Until the handshake is done
    whatever is sent is dropped, and frames still waiting when a newer one
    for the same surface or signal comes are replaced (counted in `dropped`).
    """

    def __init__(
        self,
        url: str,
        token: str,
        handler: Handler,
        on_end: Callable[[str], None],
        node: Mapping[str, str] | None = None,
    ) -> None:
        self.url = url
        self._token = token
        self.handler = handler
        self.on_end = on_end
        self.node = dict(node) if node is not None else engine_summary()
        self.backoff_first = BACKOFF_FIRST
        self.backoff_max = BACKOFF_MAX
        self.give_up = GIVE_UP
        self.dropped = 0
        self.ended = ''  # why the link gave up, if it did
        self.thread: threading.Thread | None = None
        self._lock = threading.Lock()
        self._slots: dict[SlotKey, bytes] = {}
        self._replies: deque[bytes] = deque()
        self._woken = False
        self._linked = threading.Event()
        self._had_link = False  # the last connection got past the handshake
        self._stopping = threading.Event()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._wake: asyncio.Event | None = None
        self._halt: asyncio.Event | None = None

    @property
    def connected(self) -> bool:
        """Whether the handshake is done and the node is there."""
        return self._linked.is_set()

    def wait_connected(self, timeout: float) -> bool:
        return self._linked.wait(timeout)

    def start(self) -> None:
        self.thread = threading.Thread(target=self._run, name='source-link', daemon=True)
        self.thread.start()

    def stop(self) -> None:
        """Closes the link. The caller joins `thread`."""
        self._stopping.set()
        loop, halt = self._loop, self._halt
        if loop is not None and halt is not None:
            with contextlib.suppress(RuntimeError):  # its loop has already finished
                loop.call_soon_threadsafe(halt.set)

    def send_frame(self, frame: bytes) -> None:
        """Sends a surface or signal frame; newest wins per surface and per signal."""
        key, kind = _frame_key(frame)
        self._put(key, bytes([kind]) + frame)

    def notify(self, method: str, params: Mapping[str, Any], part: int | None = None) -> None:
        """
        Sends a notification; newest wins per method, or per method and `part`
        for notifications sent once per surface.
        """
        slot = method if part is None else f'{method}:{part}'
        self._put(
            ('notify', slot), rpc_message({'jsonrpc': '2.0', 'method': method, 'params': params})
        )

    def _put(self, key: SlotKey, message: bytes) -> None:
        if not self._linked.is_set():
            return
        with self._lock:
            if self._slots.pop(key, None) is not None and key[0] != 'notify':
                self.dropped += 1
            self._slots[key] = message
            wake = not self._woken
            self._woken = True
        loop, event = self._loop, self._wake
        if wake and loop is not None and event is not None:
            with contextlib.suppress(RuntimeError):
                loop.call_soon_threadsafe(event.set)

    def _run(self) -> None:
        try:
            asyncio.run(self._main())
        except Exception:
            log.exception('the source link failed')
            self._end('failed')

    def _end(self, reason: str) -> None:
        self.ended = reason
        self.on_end(reason)

    async def _main(self) -> None:
        self._loop = asyncio.get_running_loop()
        self._wake, self._halt = asyncio.Event(), asyncio.Event()
        if self._stopping.is_set():
            return
        lost, attempt = time.monotonic(), 0
        while not self._stopping.is_set():
            self._had_link = False
            try:
                await self._connect_once()
            except Refused as refused:
                log.error('the node refused the link: %s', refused)
                self._end('refused')
                return
            except (OSError, TimeoutError, WebSocketException, LinkError) as error:
                log.info('no link to the node: %s', error or type(error).__name__)
            if self._stopping.is_set():
                return
            if self._had_link:
                log.warning('lost the node; reconnecting')
                lost, attempt = time.monotonic(), 0
            left = self.give_up - (time.monotonic() - lost)
            delay = min(self.backoff_max, self.backoff_first * 2.0**attempt)
            attempt += 1
            await self._pause(min(left, delay * (1.0 - JITTER * random.random())))
            if not self._stopping.is_set() and time.monotonic() - lost >= self.give_up:
                log.error('no node for %.0f s; giving up', self.give_up)
                self._end('lost')
                return

    async def _pause(self, seconds: float) -> None:
        """Waits, or until stop()."""
        assert self._halt is not None
        with contextlib.suppress(TimeoutError):
            await asyncio.wait_for(self._halt.wait(), max(0.0, seconds))

    async def _connect_once(self) -> None:
        """One connection: the handshake, then the session until it ends."""
        async with connect(
            self.url,
            subprotocols=[SUBPROTOCOL],
            compression=None,  # frames are binary, LZ4 where asked
            proxy=None,  # loopback never goes through a proxy
            open_timeout=OPEN_TIMEOUT,
            close_timeout=CLOSE_TIMEOUT,
            max_size=MAX_MESSAGE,
        ) as ws:
            if ws.subprotocol != SUBPROTOCOL:
                raise LinkError(f'the node does not speak {SUBPROTOCOL}')
            await self._handshake(ws)
            with self._lock:
                self._slots.clear()
                self._replies.clear()
                self._woken = False
            self._linked.set()
            self._had_link = True
            log.info('linked to the node')
            try:
                await self._session(ws)
            finally:
                self._linked.clear()

    async def _handshake(self, ws: ClientConnection) -> None:
        hello = {
            'version': [CONTRACT_VERSION],
            'node': self.node,
            'purpose': {'kind': 'local', 'token': self._token},
        }
        await ws.send(json.dumps(hello))
        raw = await asyncio.wait_for(ws.recv(), OPEN_TIMEOUT)
        try:
            welcome = json.loads(raw)
        except ValueError as error:
            raise LinkError('the handshake response is not JSON') from error
        result = cast(dict[str, Any], welcome).get('result') if isinstance(welcome, dict) else None
        if result != 'ok':
            raise Refused(str(result))

    async def _session(self, ws: ClientConnection) -> None:
        """Receives and sends until the connection ends or stop()."""
        assert self._halt is not None
        tasks: set[asyncio.Task[Any]] = {
            asyncio.create_task(self._receive(ws)),
            asyncio.create_task(self._send(ws)),
            asyncio.create_task(self._halt.wait()),
        }
        done, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        for task in done:
            error = task.exception()
            if error is not None and not isinstance(error, ConnectionClosed):
                raise error
        if self._halt.is_set():
            # Stopping: what is still waiting goes out first, the last status and replies
            with contextlib.suppress(ConnectionClosed):
                await self._flush(ws)

    async def _receive(self, ws: ClientConnection) -> None:
        assert self._wake is not None
        async for message in ws:
            if isinstance(message, str):
                continue  # only the handshake is text
            reply = self._answer(message)
            if reply is not None:
                with self._lock:
                    self._replies.append(reply)
                self._wake.set()

    async def _send(self, ws: ClientConnection) -> None:
        assert self._wake is not None
        while True:
            await self._wake.wait()
            self._wake.clear()
            await self._flush(ws)

    async def _flush(self, ws: ClientConnection) -> None:
        """Sends the replies, then whatever waits in the slots."""
        with self._lock:
            messages = [*self._replies, *self._slots.values()]
            self._replies.clear()
            self._slots = {}
            self._woken = False
        for message in messages:
            await ws.send(message)

    def _answer(self, message: bytes) -> bytes | None:
        """The reply to a message from the node, if it needs one."""
        if not message or message[0] != TYPE_RPC:
            log.debug('ignored a message of type %s', message[:1].hex())
            return None
        try:
            request = json.loads(message[1:])
        except ValueError:
            log.warning('ignored a JSON-RPC message that is not JSON')
            return None
        if not isinstance(request, dict):
            return None
        request = cast(dict[str, Any], request)
        if 'method' not in request or 'id' not in request:
            return None  # a response or a notification: the engine expects neither
        rid = request['id']
        method, params = request.get('method'), request.get('params', {})
        if not isinstance(rid, int | str) or isinstance(rid, bool):
            log.warning('ignored a request with a bad id')
            return None
        if request.get('jsonrpc') != '2.0' or not isinstance(method, str):
            return self._error(rid, INVALID_REQUEST, 'not a JSON-RPC 2.0 request')
        if not isinstance(params, dict):
            return self._error(rid, INVALID_PARAMS, 'params must be an object')
        try:
            result = self.handler(method, cast(dict[str, Any], params))
        except RpcError as error:
            return self._error(rid, error.code, error.message)
        except Exception:
            log.exception('answering %s failed', method)
            return self._error(rid, INTERNAL_ERROR, 'internal error')
        return rpc_message({'jsonrpc': '2.0', 'id': rid, 'result': result})

    @staticmethod
    def _error(rid: int | str, code: int, text: str) -> bytes:
        return rpc_message({'jsonrpc': '2.0', 'id': rid, 'error': {'code': code, 'message': text}})
