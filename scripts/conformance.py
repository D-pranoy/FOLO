"""
The conformance harness (RB-02 step 5): drives an implementation of FOLO's
API or WebSocket channels against the contracts in specs/.

    uv run python scripts/conformance.py api --url http://127.0.0.1:8777/api/v1 [--key KEY] [st ...]
    uv run python scripts/conformance.py api --node target/debug/folo [st ...]
    uv run python scripts/conformance.py messages DIR

`api` runs schemathesis against specs/openapi.yaml: every operation with
generated requests, every response checked against the spec, with every
check but those scripts/schemathesis.toml says the client API's documented
answers rule out. Point it at a node of its own on loopback
(docs/process/testing-policy.md), never at a live setup. With `--node`, the
harness starts that `folo` itself, with a temporary home, a free loopback
port and no effect engine, and stops it afterwards; CI runs it so.

schemathesis exits 0 with errored cases (a check that errored, or a case that
got no response), so the harness reads its NDJSON report, prints each one and
fails unless EXPECTED_ERRORS lists its phase with a reason.

`messages` checks captured WebSocket messages against specs/asyncapi.yaml.
DIR holds one message per file, named after its AsyncAPI message
(components/messages), optionally with --<anything> before the extension:
`config.apply.json`, `subscribe--empty.json`, `surfaceFrame--rgb8.fols`.
JSON files are checked against the message's payload schema; `.fols`, `.folg`
and `.folc` files are surface, signal and screen frames, checked against their layout
(specs/schema/source-link.json). An implementation's tests can dump what it
sends into a folder and run this. CI runs it on specs/fixtures/.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
from collections.abc import Generator, Iterator
from contextlib import contextmanager
from functools import cache
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import yaml
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from referencing.jsonschema import DRAFT202012

ROOT = Path(__file__).resolve().parent.parent
SPECS = ROOT / 'specs'
# How schemathesis treats the client API's documented answers
SCHEMATHESIS_CONFIG = ROOT / 'scripts' / 'schemathesis.toml'
OPENAPI = SPECS / 'openapi.yaml'
ASYNCAPI = SPECS / 'asyncapi.yaml'
SOURCE_LINK = SPECS / 'schema' / 'source-link.json'
FIXTURES = SPECS / 'fixtures'

Json = Any
PIXEL_BYTES = {1: 3, 2: 6, 3: 12}  # rgb8, rgb16, rgbf32: bytes per cell
SCREEN_MAX = (480, 270)  # the widest and highest screen frame the capture agent sends
NODE_START_S = 30.0  # how long a node of its own has to answer /health
NODE_STOP_S = 10.0
# Errored cases schemathesis reports that are expected, by phase, with the
# reason. Anything else fails the run: an errored case checked nothing.
EXPECTED_ERRORS = {
    'stateful': (
        'schemathesis generates the last step of a stateful scenario and does not '
        'send it; the node answers the same requests normally'
    ),
}


# ---------------------------------------------------------------- API


def errored_cases(report: Path) -> list[tuple[str, str]]:
    """
    The cases schemathesis counts as errored in its NDJSON report: a check
    that errored, or a case that got no response. Returns (phase, detail).
    """
    errored: list[tuple[str, str]] = []
    for line in report.read_text(encoding='utf-8').splitlines():
        event: dict[str, Json] = json.loads(line).get('ScenarioFinished') or {}
        recorder: dict[str, Json] = event.get('recorder') or {}
        if 'cases' not in recorder:
            continue
        for case_id, case in recorder['cases'].items():
            checks: list[Json] = recorder.get('checks', {}).get(case_id, [])
            sent = case_id in recorder.get('interactions', {})
            failed: list[Json] = [check for check in checks if check['status'] == 'error']
            if sent and not failed:
                continue
            value: Json = case['value']
            why = 'no response' if not sent else f'check {failed[0]["name"]} errored'
            errored.append((event['phase'], f'{value["method"]} {value["path"]}: {why}'))
    return errored


def unexpected_errors(errored: list[tuple[str, str]]) -> list[str]:
    """Prints every errored case; returns those not listed in EXPECTED_ERRORS."""
    unexpected: list[str] = []
    for phase, detail in errored:
        reason = EXPECTED_ERRORS.get(phase)
        print(f'errored ({phase}): {detail}' + (f' [expected: {reason}]' if reason else ''))
        if reason is None:
            unexpected.append(f'{phase}: {detail}')
    return unexpected


def run_api(url: str, key: str | None, extra: list[str]) -> int:
    st = shutil.which('st') or shutil.which('schemathesis')
    if st is None:
        raise SystemExit('schemathesis is needed: uv sync')
    with tempfile.TemporaryDirectory(prefix='folo-conformance-report-') as folder:
        report = Path(folder) / 'events.ndjson'
        cmd = [
            st,
            '--config-file',
            str(SCHEMATHESIS_CONFIG),
            'run',
            str(OPENAPI),
            '--url',
            url,
            '--exclude-tag',
            'websockets',
            '--no-color',
            '--report',
            'ndjson',
            '--report-ndjson-path',
            str(report),
        ]
        if key:
            cmd += ['--header', f'Authorization: Bearer {key}']
        # Its report has non-ASCII characters, which a Windows console can't take otherwise
        env = {**os.environ, 'PYTHONUTF8': '1', 'PYTHONIOENCODING': 'utf-8'}
        status = subprocess.run(cmd + extra, check=False, env=env).returncode
        # schemathesis exits 0 with errored cases, which would pass silently
        if not report.exists():
            print('schemathesis wrote no NDJSON report (--report* arguments?)', file=sys.stderr)
            return status or 1
        unexpected = unexpected_errors(errored_cases(report))
    if unexpected:
        print(f'{len(unexpected)} errored case(s) not listed in EXPECTED_ERRORS', file=sys.stderr)
        return status or 1
    return status


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return int(probe.getsockname()[1])


def _healthy(url: str) -> bool:
    try:
        with urllib.request.urlopen(f'{url}/health', timeout=2) as answer:
            return answer.status == 200
    except OSError:
        return False


@contextmanager
def own_node(folo: Path) -> Generator[str]:
    """
    A node of its own: `folo` with a temporary home, a free loopback port
    and no effect engine, stopped afterwards. Yields its API base.
    """
    temporary = tempfile.TemporaryDirectory(prefix='folo-conformance-', ignore_cleanup_errors=True)
    with temporary as home:
        port = _free_port()
        cmd = [str(folo.resolve()), 'run', '--home', home, '--port', str(port)]
        cmd += ['--local-source', 'off', '--log-level', 'warn']
        # Run in its home: a first run looks for studio.json in the working
        # directory too, and a live one must never be imported
        node = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, cwd=home)
        url = f'http://127.0.0.1:{port}/api/v1'
        try:
            deadline = time.monotonic() + NODE_START_S
            while not _healthy(url):
                if node.poll() is not None:
                    raise SystemExit(f'{folo} exited with status {node.returncode}')
                if time.monotonic() > deadline:
                    raise SystemExit(f'{folo} did not answer within {NODE_START_S:.0f} s')
                time.sleep(0.1)
            yield url
        finally:
            node.terminate()
            try:
                node.wait(timeout=NODE_STOP_S)
            except subprocess.TimeoutExpired:
                node.kill()
                node.wait()


# ---------------------------------------------------------------- messages


def _load(path: Path) -> Json:
    text = path.read_text(encoding='utf-8')
    return yaml.safe_load(text) if path.suffix in ('.yaml', '.yml') else json.loads(text)


def _retrieve(uri: str) -> Resource[Json]:
    """Schemas referenced from the AsyncAPI document: local files only."""
    if urlparse(uri).scheme != 'file':
        raise ValueError(f'only local files are read: {uri}')
    return Resource.from_contents(_load(Path.from_uri(uri)), default_specification=DRAFT202012)


@cache
def _registry() -> Registry[Json]:
    asyncapi = Resource.from_contents(_load(ASYNCAPI), default_specification=DRAFT202012)
    return Registry(retrieve=_retrieve).with_resource(ASYNCAPI.as_uri(), asyncapi)  # type: ignore[call-arg]


@cache
def message_names() -> tuple[str, ...]:
    return tuple(_load(ASYNCAPI)['components']['messages'])


@cache
def _validator(message: str) -> Draft202012Validator:
    pointer = f'{ASYNCAPI.as_uri()}#/components/messages/{message}/payload'
    return Draft202012Validator({'$ref': pointer}, registry=_registry())


@cache
def _layout(name: str) -> tuple[list[dict[str, Json]], int]:
    header = json.loads(SOURCE_LINK.read_text(encoding='utf-8'))['$defs'][name]
    return header['x-folo-layout'], header['x-folo-header-size']


def _fields(data: bytes, layout: list[dict[str, Json]]) -> dict[str, Json]:
    formats = {1: '<B', 2: '<H', 4: '<I', 8: '<Q'}
    out: dict[str, Json] = {}
    for field in layout:
        raw = data[field['offset'] : field['offset'] + field['size']]
        name = field['field']
        value = (
            raw.decode('ascii', 'replace')
            if name == 'magic'
            else struct.unpack(formats[field['size']], raw)[0]
        )
        if name == 'reserved':
            out.setdefault('reserved', []).append(value)
        else:
            out[name] = value
    return out


_HEADERS = {
    'surface': 'SurfaceFrameHeader',
    'signal': 'SignalFrameHeader',
    'screen': 'ScreenFrameHeader',
}


def _check_screen_frame(fields: dict[str, Json], payload: int) -> list[str]:
    problems: list[str] = []
    if fields['pixel_format'] != 1:
        return [f'unknown pixel format {fields["pixel_format"]}']
    if fields['flags'] & ~0b111:
        problems.append(f'unknown flags {fields["flags"]:#04x}')
    width, height = fields['width'], fields['height']
    if width < 1 or height < 1:
        problems.append('width and height must be at least 1')
    elif width > SCREEN_MAX[0] or height > SCREEN_MAX[1]:
        problems.append(f'{width} x {height} cells are wider than 480 or higher than 270')
    if payload != width * height * 3:
        problems.append(
            f'payload is {payload} bytes; {width} x {height} cells need {width * height * 3}'
        )
    if fields['bars_top'] + fields['bars_bottom'] >= max(height, 1):
        problems.append('the top and bottom bars leave no row')
    if fields['bars_left'] + fields['bars_right'] >= max(width, 1):
        problems.append('the left and right bars leave no column')
    return problems


def check_frame(data: bytes, kind: str) -> list[str]:
    """A surface (FOLS), signal (FOLG) or screen (FOLC) frame against its layout."""
    layout, size = _layout(_HEADERS[kind])
    if len(data) < size:
        return [f'shorter than the {size}-byte header']
    fields = _fields(data, layout)
    problems: list[str] = []
    for field in layout:
        if (
            'value' in field
            and field['field'] != 'reserved'
            and fields[field['field']] != field['value']
        ):
            problems.append(
                f'{field["field"]} is {fields[field["field"]]!r}, not {field["value"]!r}'
            )
    if any(value != 0 for value in fields.get('reserved', [])):
        problems.append('a reserved field is not 0')
    payload = len(data) - size
    if kind == 'screen':
        return [*problems, *_check_screen_frame(fields, payload)]
    if kind == 'surface':
        fmt = fields['pixel_format']
        if fmt not in PIXEL_BYTES:
            return [*problems, f'unknown pixel format {fmt}']
        if fields['flags'] & ~0b11:
            problems.append(f'unknown flags {fields["flags"]:#04x}')
        lz4 = fields['flags'] & 1
        expected = fields['width'] * fields['height'] * PIXEL_BYTES[fmt]
        if not lz4 and payload != expected:
            problems.append(
                f'payload is {payload} bytes; '
                f'{fields["width"]} x {fields["height"]} cells need {expected}'
            )
        if fields['width'] < 1 or fields['height'] < 1:
            problems.append('width and height must be at least 1')
    elif payload != 4 * fields['count']:
        problems.append(
            f'payload is {payload} bytes; '
            f'{fields["count"]} float32 values need {4 * fields["count"]}'
        )
    return problems


FRAME_FILES = {'.fols': 'surfaceFrame', '.folg': 'signalFrame', '.folc': 'screenFrame'}


def message_of(path: Path) -> str:
    return path.name.split('--')[0].removesuffix(path.suffix)


def check_message_file(path: Path) -> list[str]:
    message = message_of(path)
    if path.suffix in FRAME_FILES:
        expected = FRAME_FILES[path.suffix]
        if message != expected:
            return [f'{path.name}: {path.suffix} files hold {expected} messages']
        return [
            f'{path.name}: {p}'
            for p in check_frame(path.read_bytes(), expected.removesuffix('Frame'))
        ]
    if message not in message_names():
        return [f'{path.name}: no AsyncAPI message is called {message}']
    try:
        payload = json.loads(path.read_text(encoding='utf-8'))
    except ValueError as exc:
        return [f'{path.name}: not JSON: {exc}']
    problems: list[str] = []
    for error in _validator(message).iter_errors(payload):  # pyright: ignore[reportUnknownMemberType]
        where = '/'.join(str(part) for part in error.path) or '(root)'
        problems.append(f'{path.name}: {where}: {error.message}')
    return sorted(problems)[:5]


def message_files(folder: Path) -> Iterator[Path]:
    return iter(
        sorted(p for p in folder.iterdir() if p.suffix == '.json' or p.suffix in FRAME_FILES)
    )


def check_messages(folder: Path) -> list[str]:
    problems: list[str] = []
    files = list(message_files(folder))
    if not files:
        return [f'{folder}: no message files']
    for path in files:
        problems += check_message_file(path)
    return problems


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    commands = parser.add_subparsers(dest='command', required=True)
    api = commands.add_parser('api', help='run schemathesis against a running API')
    target = api.add_mutually_exclusive_group(required=True)
    target.add_argument('--url', help='the API base, for example http://127.0.0.1:8777/api/v1')
    target.add_argument(
        '--node', type=Path, help='a folo executable to start and test on a home of its own'
    )
    api.add_argument('--key', help='a client key (loopback needs none)')
    messages = commands.add_parser('messages', help='check captured WebSocket messages')
    messages.add_argument('folder', type=Path, nargs='?', default=FIXTURES)
    args, extra = parser.parse_known_args()
    if args.command == 'api':
        if args.node is None:
            sys.exit(run_api(args.url, args.key, extra))
        with own_node(args.node) as url:
            status = run_api(url, args.key, extra)
        sys.exit(status)
    problems = check_messages(args.folder)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        sys.exit(f'{len(problems)} problem(s)')
    print(f'{args.folder}: every message conforms')


if __name__ == '__main__':
    main()
