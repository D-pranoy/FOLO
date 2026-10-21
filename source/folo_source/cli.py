"""
The `folo-source` command: the effect engine, as its node starts it with the
local link's URL and launch token in its environment (ADR-0032). Exit status:
0 after a stop or a restart the node asked for, 1 when the link was refused
or the node was lost, 2 when the environment lacks the link.
"""

from __future__ import annotations

import argparse
import logging
import os
from collections.abc import Sequence

from folo_source.publish.link import ENV_TOKEN, LinkError, link_settings
from folo_source.runtime import lifecycle, logs
from folo_source.runtime.source import SourceEngine

log = logging.getLogger('folo_source')

EXIT_OK = 0
EXIT_LINK = 1
EXIT_USAGE = 2
# Why the engine may end on its own, and whether that is a failure
FAILURES = ('refused', 'lost', 'failed')


def parser() -> argparse.ArgumentParser:
    out = argparse.ArgumentParser(
        prog='folo-source',
        description="FOLO's effect engine. Its node starts it; it isn't run by hand.",
    )
    out.add_argument(
        '--seed', type=int, help='seed for the random modes (drops, fire), for repeatable output'
    )
    return out


def run(seed: int | None) -> int:
    """Runs the engine until it is stopped or its link ends."""
    try:
        url, token = link_settings(os.environ)
    except LinkError as error:
        log.error('%s', error)
        return EXIT_USAGE
    os.environ.pop(ENV_TOKEN, None)  # nothing this process starts inherits it
    shutdown = lifecycle.Shutdown()
    shutdown.install()
    ended: list[str] = []

    def on_end(reason: str) -> None:
        ended.append(reason)
        shutdown.request(f'the link ended: {reason}')

    engine = SourceEngine(url, token, on_end, seed=seed)
    engine.start()
    try:
        shutdown.wait()
    finally:
        engine.stop()
        log.info('stopped')
        shutdown.done()
    return EXIT_LINK if ended and ended[0] in FAILURES else EXIT_OK


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    logs.setup()
    return run(args.seed)
