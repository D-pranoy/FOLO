"""
Golden-frame cases for the current engine (RB-01 step 6): what is rendered,
and the harness that renders it the way the engine does (AudioInput.analyse,
then StudioEngine._on_packet).

Every case is a v1 config (sanitized like the studio's own), a signal from
tests/synth.py and the audio settings. The spectra depend only on the
signal and the audio settings, so they are analysed once per signal and
shared by the cases; test_golden_renderer checks this against the real
StudioEngine._on_packet path.

Frames are stored as PNGs (time down, LEDs across), byte for byte the
renderer's output, so they can also be looked at.
"""

from __future__ import annotations

import hashlib
import json
import struct
import zlib
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np
import numpy.typing as npt

from folo_source.effects.visualizers import Renderer, shown_levels
from folo_source.inputs.audio.analyzer import (
    SAMPLERATE,
    Analyzer,
    AutoLevel,
    Spectrum,
    channel_peaks,
)
from folo_source.legacy import v1 as studio_config
from folo_source.runtime.engine import IDLE_AFTER, IDLE_FPS, MAX_DT, StudioEngine
from folo_source.signals.beat import ONSET_DELAY, BeatTracker
from tests import synth

GOLDEN_DIR = Path(__file__).parent / 'golden'
RENDERER_DIR = GOLDEN_DIR / 'renderer'
MANIFEST = RENDERER_DIR / 'manifest.json'
BUILTINS = GOLDEN_DIR / 'builtins.json'

SEED = 1234  # the renderer's random generator (drops, fire)
IDLE_SECONDS = 3.0  # rendering without audio packets after the signal, for ambient cases

Config = dict[str, Any]
Frames = npt.NDArray[np.uint8]

# Deep Night palettes (AGENTS.md conventions)
LOWS = ['#0033CC', '#000080', '#4B00FF']
MIDS = ['#FF3399', '#FF00FF', '#800080']
HIGHS = ['#800080', '#8F00FF', '#4B0082']
FULL = ['#000080', '#4B00FF', '#8F00FF', '#FF00FF', '#FF3399']


@dataclass
class Case:
    name: str
    signal: str
    config: Config
    colors: tuple[str, ...] = ()  # album art colors replacing the palettes
    paused: bool = False  # the player says it's paused
    idle: float = 0.0  # seconds of idle rendering after the signal
    builtin: tuple[str, int] | None = None  # (id, led count) of a built-in look, see builtins.json

    @property
    def analysis(self) -> tuple[str, float, bool]:
        """What the spectra depend on: the signal, input gain and auto level."""
        audio = self.config['audio']
        return (self.signal, audio['gain'], audio['auto_level'])

    def describe(self) -> dict[str, Any]:
        described: dict[str, Any] = {
            'signal': self.signal,
            'colors': list(self.colors),
            'paused': self.paused,
            'idle': self.idle,
        }
        if self.builtin is not None:
            # make_config(led_count, segments from builtins.json)
            described['builtin'] = {'id': self.builtin[0], 'led_count': self.builtin[1]}
        else:
            described['config'] = self.config
        return described


@dataclass
class Result:
    frames: Frames  # packets x leds x rgb
    levels: list[Any] = field(
        default_factory=list[Any]
    )  # shown_levels() per frame, by segment order

    def frames_sha256(self) -> str:
        return hashlib.sha256(self.frames.tobytes()).hexdigest()

    def levels_sha256(self) -> str:
        text = json.dumps(self.levels, separators=(',', ':'))
        return hashlib.sha256(text.encode()).hexdigest()


ProgressFn = Callable[[int, int, str], None]


# ---------------------------------------------------------------- configs


def make_config(led_count: int, segments: list[Config], **top: Any) -> Config:
    """A sanitized v1 config with numbered segment ids."""
    cfg = studio_config.default_config(led_count)
    cfg.update(top)
    cfg['segments'] = [dict(seg, id=f's{i}') for i, seg in enumerate(segments)]
    clean = studio_config.sanitize_config(cfg)
    clean['hyperion']['enabled'] = False
    return clean


def segment(start: int, end: int, band: str, trigger: str, **fields: Any) -> Config:
    lo, hi, palette = {
        'lows': (30.0, 150.0, LOWS),
        'mids': (250.0, 2000.0, MIDS),
        'highs': (4000.0, 16000.0, HIGHS),
        'full': (30.0, 16000.0, FULL),
    }[band]
    threshold, ceiling = {'level': (-60.0, -20.0), 'hits': (2.0, 12.0), 'beat': (-70.0, -30.0)}[
        trigger
    ]
    seg: Config = {
        'name': f'{band} {fields.get("direction", "forward")}',
        'start': start,
        'end': end,
        'freq_lo': lo,
        'freq_hi': hi,
        'trigger': trigger,
        'threshold': threshold,
        'ceiling': ceiling,
        'attack': 10.0,
        'release': 200.0,
        'palette': palette,
    }
    seg.update(fields)
    return seg


def mode_cases() -> list[Case]:
    """
    Every mode x trigger (level, hits, beat with each `beats`), one segment
    per direction on its own band, with floor and peak variants.
    """
    cases: list[Case] = []
    triggers = [('level', None), ('hits', None)] + [
        ('beat', b) for b in studio_config.BEAT_DIVISIONS
    ]
    bands = ('lows', 'mids', 'highs', 'full')
    for mode in studio_config.MODES:
        for trigger, beats in triggers:
            variants: list[tuple[str, dict[str, Any]]] = [('', {}), ('-floor', {'floor': 0.3})]
            if mode == 'fill':
                variants.append(('-peak', {'peak': True}))
            for suffix, extra in variants:
                segments = [
                    segment(
                        i * 20,
                        i * 20 + 19,
                        band,
                        trigger,
                        mode=mode,
                        direction=direction,
                        **({'beats': beats} if beats else {}),
                        **extra,
                    )
                    for i, (direction, band) in enumerate(
                        zip(studio_config.DIRECTIONS, bands, strict=True)
                    )
                ]
                label = f'{trigger}{"" if beats is None else f"-{beats:g}"}'
                cases.append(
                    Case(
                        f'mode-{mode}-{label}{suffix}',
                        'song' if trigger == 'beat' else 'medley',
                        make_config(80, segments),
                    )
                )
    return cases


def level_cases() -> list[Case]:
    """Input gain and auto level, up to +54 dB (24 dB gain plus 30 dB auto level)."""
    segments = studio_config.default_config(150)['segments']

    def cfg(gain: float, auto: bool) -> Config:
        return make_config(150, segments, audio={'gain': gain, 'auto_level': auto})

    cases = [Case(f'gain{gain:+g}', 'medley', cfg(gain, False)) for gain in (-24.0, 0.0, 24.0)]
    cases.append(Case('gain+0-auto', 'medley', cfg(0.0, True)))
    cases.append(Case('gain+24-auto-quiet', 'medley_quiet', cfg(24.0, True)))
    return cases


def look_cases() -> list[Case]:
    """Ambient light, master, background, solo, album colors and the beat offset."""
    segs = [
        segment(0, 39, 'lows', 'hits', mode='pulse'),
        segment(40, 79, 'mids', 'level', mode='fill', direction='center'),
        segment(80, 119, 'highs', 'level', mode='spectrum'),
    ]
    cases: list[Case] = []
    for reactive in ('auto', 'off'):
        for style in ('solid', 'breathe'):
            cfg = make_config(
                120,
                segs,
                reactive=reactive,
                ambient={'color': '#4B00FF', 'style': style, 'level': 0.5, 'idle_after': 1.0},
            )
            cases.append(Case(f'ambient-{reactive}-{style}', 'medley', cfg, idle=IDLE_SECONDS))
    paused = make_config(
        120,
        segs,
        reactive='auto',
        ambient={'color': '#FF3399', 'style': 'solid', 'level': 0.4, 'idle_after': 30.0},
    )
    cases.append(Case('ambient-auto-paused', 'medley', paused, paused=True, idle=IDLE_SECONDS))
    cases.append(
        Case(
            'master-background', 'medley', make_config(120, segs, master=0.45, background='#101020')
        )
    )
    solo = [
        dict(segs[0], solo=True),
        dict(segs[1], enabled=False),
        segs[2],
        segment(30, 90, 'full', 'level', mode='fill', solo=True),
    ]
    cases.append(Case('solo-disabled', 'medley', make_config(120, solo)))
    cases.append(
        Case(
            'cover-colors',
            'medley',
            make_config(120, segs),
            colors=('#FF0000', '#00FF00', '#0000FF', '#FFFF00'),
        )
    )
    beat = [
        segment(0, 59, 'lows', 'beat', mode='pulse'),
        segment(60, 119, 'full', 'beat', mode='wipe', beats=4.0),
    ]
    for offset in (-150, 100):
        cases.append(
            Case(f'beat-offset{offset:+d}', 'song', make_config(120, beat, beat={'offset': offset}))
        )
    return cases


def builtin_cases(builtins: dict[str, Any] | None = None) -> list[Case]:
    """Every built-in look of the web UI (scripts/export_builtins.mjs) at each LED count."""
    data = builtins if builtins is not None else json.loads(BUILTINS.read_text(encoding='utf-8'))
    cases: list[Case] = []
    for look in data['builtins']:
        for count, segments in look['segments'].items():
            cfg = make_config(int(count), segments)
            # The page posts the segments with the ids it made
            for seg, original in zip(cfg['segments'], segments, strict=True):
                seg['id'] = original['id']
            name = look['id'].replace(':', '-')
            cases.append(
                Case(f'builtin-{name}-{count}', 'song', cfg, builtin=(look['id'], int(count)))
            )
    return cases


def all_cases() -> list[Case]:
    cases = mode_cases() + level_cases() + look_cases() + builtin_cases()
    names = [c.name for c in cases]
    if len(names) != len(set(names)):
        raise ValueError('duplicate golden case names')
    return cases


# ---------------------------------------------------------------- rendering


@dataclass
class Packet:
    spectrum: Spectrum
    frames: int
    dt: float


_signals: dict[str, synth.Capture] = {}
_analyses: dict[tuple[str, float, bool], list[Packet]] = {}


def signal(name: str) -> synth.Capture:
    if name not in _signals:
        _signals[name] = synth.SIGNALS[name]()
    return _signals[name]


def signal_sha256(name: str) -> str:
    return hashlib.sha256(synth.pcm16(signal(name))).hexdigest()


def analyse(key: tuple[str, float, bool]) -> list[Packet]:
    """
    The spectra AudioInput.analyse works out for (signal, gain, auto
    level): the input gain set before each packet, auto level following
    after it. The short window is worked out now (the engine does it when a
    hits segment asks; the result is the same), so a packet doesn't keep its
    samples around.
    """
    name, gain, auto_level = key
    if key not in _analyses:
        _analyses.clear()  # one at a time: callers go through cases grouped by analysis
        analyzer = Analyzer()
        level = AutoLevel()
        out: list[Packet] = []
        for frames in synth.packets(signal(name)):
            analyzer.offset = gain + (level.gain if auto_level else 0.0)
            spectrum = analyzer.push(frames, channel_peaks(frames))
            dt = min(MAX_DT, len(frames) / float(SAMPLERATE))
            level.update(spectrum.loudest, dt)
            shared = Spectrum(
                spectrum.padded_for(False), spectrum.padded_for(True), spectrum.offset
            )
            out.append(Packet(shared, len(frames), dt))
        _analyses[key] = out
    return _analyses[key]


def idle_steps(seconds: float) -> list[float]:
    """
    The dt of each idle render (StudioEngine._idle_loop): the first after
    IDLE_AFTER, then IDLE_FPS a second.
    """
    if seconds <= 0:
        return []
    steps = [IDLE_AFTER]
    while sum(steps) < seconds:
        steps.append(1.0 / IDLE_FPS)
    return steps


def uses_beat(config: Config) -> bool:
    return any(s['trigger'] == 'beat' and s['enabled'] for s in config['segments'])


def render(case: Case) -> Result:
    """Renders a case from the shared spectra, packet by packet like the engine."""
    config = case.config
    analyzer = Analyzer()
    renderer = Renderer(analyzer, rng=np.random.default_rng(SEED))
    beat = BeatTracker(analyzer.freqs, SAMPLERATE)
    tracking = uses_beat(config)
    order = [s['id'] for s in config['segments']]
    frames: list[bytes] = []
    levels: list[Any] = []

    def step(spectrum: Spectrum, count: int, dt: float) -> None:
        # StudioEngine._track_beat, without a page watching
        if tracking:
            beat.lead = ONSET_DELAY + config['beat']['offset'] / 1000.0
            beat.push(spectrum, count)
        else:
            beat.skip()
        frame, seg_levels = renderer.render(
            config, spectrum, dt, beat=beat, colors=case.colors, paused=case.paused
        )
        frames.append(frame)
        shown = shown_levels(seg_levels)
        levels.append([shown[i] for i in order])

    for packet in analyse(case.analysis):
        step(packet.spectrum, packet.frames, packet.dt)
    for dt in idle_steps(case.idle):
        step(Spectrum.silent(), int(dt * SAMPLERATE), dt)
    count = config['led_count']
    data = np.frombuffer(b''.join(frames), dtype=np.uint8).reshape(len(frames), count, 3)
    return Result(data, levels)


# ---------------------------------------------------------------- the real engine


class FakeStore:
    """What StudioEngine reads of the ConfigStore."""

    def __init__(self, config: Config) -> None:
        self.config = config
        self.revision = 1
        self.server_id = 'golden'


def render_with_engine(case: Case) -> Frames:
    """
    Renders a case through AudioInput.analyse and StudioEngine._on_packet (no threads, no
    devices), to check that render() mirrors it. Not for idle or paused cases.
    """
    if case.idle or case.paused or case.colors:
        raise ValueError('render_with_engine only covers audio packets')
    engine = StudioEngine(FakeStore(case.config))
    engine.renderer.rng = np.random.default_rng(SEED)
    engine._watched = float('-inf')  # pyright: ignore[reportPrivateUsage]  # no page is watching
    frames: list[bytes] = []
    for packet in synth.packets(signal(case.signal)):
        analysed = engine.input.analyse(packet, channel_peaks(packet), False)
        engine._on_packet(analysed)  # pyright: ignore[reportPrivateUsage]
        frames.append(engine.frame)
    count = case.config['led_count']
    return np.frombuffer(b''.join(frames), dtype=np.uint8).reshape(len(frames), count, 3)


# ---------------------------------------------------------------- PNG


def _chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def write_png(path: Path, frames: Frames) -> None:
    """8-bit RGB, one row per frame. Rows use the Up filter: frames change little."""
    height, width, _ = frames.shape
    rows = frames.reshape(height, width * 3).astype(np.int16)
    up = np.empty_like(rows)
    up[0] = rows[0]
    up[1:] = rows[1:] - rows[:-1]
    raw = np.empty((height, width * 3 + 1), dtype=np.uint8)
    raw[:, 0] = 2  # Up
    raw[:, 1:] = (up % 256).astype(np.uint8)
    png = (
        b'\x89PNG\r\n\x1a\n'
        + _chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
        + _chunk(b'IDAT', zlib.compress(raw.tobytes(), 9))
        + _chunk(b'IEND', b'')
    )
    path.write_bytes(png)


def read_png(path: Path) -> Frames:
    """Reads what write_png wrote (8-bit RGB, None or Up filtered rows)."""
    data = path.read_bytes()
    if data[:8] != b'\x89PNG\r\n\x1a\n':
        raise ValueError(f'{path} is not a PNG')
    pos, width, height, idat = 8, 0, 0, b''
    while pos < len(data):
        (length,) = struct.unpack('>I', data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if kind == b'IHDR':
            width, height, depth, color = struct.unpack('>IIBB', body[:10])
            if (depth, color) != (8, 2):
                raise ValueError(f'{path}: only 8-bit RGB is supported')
        elif kind == b'IDAT':
            idat += body
        pos += 12 + length
    raw = np.frombuffer(zlib.decompress(idat), dtype=np.uint8).reshape(height, width * 3 + 1)
    rows = raw[:, 1:].astype(np.int64)
    for y in range(height):
        if raw[y, 0] == 2 and y:
            rows[y] = (rows[y] + rows[y - 1]) % 256
        elif raw[y, 0] not in (0, 2):
            raise ValueError(f'{path}: unsupported PNG filter {raw[y, 0]}')
    return rows.astype(np.uint8).reshape(height, width, 3)


# ---------------------------------------------------------------- manifest


def manifest_entry(case: Case, result: Result) -> dict[str, Any]:
    return {
        **case.describe(),
        'png': f'{case.name}.png',
        'frames': int(result.frames.shape[0]),
        'frames_sha256': result.frames_sha256(),
        'levels_sha256': result.levels_sha256(),
    }


def load_manifest() -> dict[str, Any]:
    return json.loads(MANIFEST.read_text(encoding='utf-8'))


def by_name(cases: Iterable[Case]) -> dict[str, Case]:
    return {c.name: c for c in cases}


def render_all(
    cases: Iterable[Case], progress: ProgressFn | None = None
) -> Iterator[tuple[Case, Result]]:
    """Renders cases grouped by their analysis, keeping one analysis in memory at a time."""
    ordered = sorted(cases, key=lambda c: (c.analysis, c.name))
    for i, case in enumerate(ordered):
        if progress is not None:
            progress(i, len(ordered), case.name)
        result = render(case)
        if i + 1 == len(ordered) or ordered[i + 1].analysis != case.analysis:
            _analyses.clear()
        yield case, result
