"""
The audio-visualizer effect (RB-04 step 4): turns the spectrum into colours
along each layer's line, with today's segment semantics (band, trigger,
shaping, smoothing and the modes), the state of the moving modes and the
beat trigger. The surface blends the lines into its stack.
"""

import math
from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np
import numpy.typing as npt

from folo_source.contracts import clamp
from folo_source.effects.base import EffectInfo, LayerSpec, Levels, Line, register
from folo_source.effects.colors import cover_palette, palette_colors
from folo_source.inputs.audio.analyzer import Spectrum, smoothing

EFFECT_ID = 'audio-visualizer'
VERSION = '1.0.0'

AVERAGE_MS = 1000  # "recent average" that hits are measured against
PEAK_HOLD = 0.5  # seconds the peak marker of fill mode holds
PEAK_FALL = 0.8  # peak marker fall speed in line lengths/second

# Moving modes ported from LedFx effects. They draw colors per step
# themselves; drops and wipe fire on hits (the level jumping past HIT_ON
# after falling below HIT_OFF).
MOTION_MODES = ('scroll', 'drops', 'scan', 'wipe', 'flow', 'fire')
HIT_ON = 0.5
HIT_OFF = 0.25
HIT_GAP = 0.06  # shortest time between two hits of a layer
MAX_DROPS = 32

# Beat trigger: a layer flashes on the beats of its BeatTracker, as bright as
# its band has been lately (its level, holding its peaks for about a beat)
BEAT_PULSE = 0.04  # seconds a beat holds full brightness at least, so a frame sent shows it
BEAT_HOLD_FALL = 12.0  # dB per second the held level falls


class LayerState:
    """What a layer keeps from frame to frame."""

    def __init__(self):
        self.value = 0.0
        self.peak = 0.0
        self.peak_age = 0.0
        self.bands = None
        self.average = None
        self.shown_average = None  # hits: average of the level the UI's spectrum shows
        self.band_average = None
        # moving modes
        self.armed = True
        self.since_hit = 1.0
        self.buffer = None  # scroll trail colors or fire heat, per step
        self.carry = 0.0  # fraction of a step not moved yet
        self.drops = []  # [center step, age s, rgb, strength]
        self.pos = 0.0  # scan eye start, wipe progress or flow phase
        self.heading = 1.0
        self.color_index = 0
        # beat trigger
        self.held = None  # the band's level, holding its peaks
        self.pulse = 0.0  # seconds the current beat still shows


class _Part:
    """
    What the effect works out once per configuration for a layer.
    """

    __slots__ = (
        'bands',
        'beats',
        'colors',
        'fast_index',
        'hits',
        'id',
        'index',
        'length',
        'pos',
        'seg',
        'steps',
        'visible',
    )

    def __init__(self, seg, layer_id, index):
        self.seg = seg  # its parameters
        self.id = layer_id
        self.index = index  # in the full window's levels, and in the lines returned
        self.hits = seg['trigger'] == 'hits'
        # beat trigger: every this many beats
        self.beats = seg['beats'] if seg['trigger'] == 'beat' else 0.0
        self.fast_index: int | None = None  # in the short window's levels (hits)
        self.visible: bool = False
        self.length: int = 0  # cells in its line
        self.pos: npt.NDArray[np.int_] | None = None
        self.steps: int = 0
        # static gradient of fill, pulse and spectrum
        self.colors: npt.NDArray[np.float64] | None = None
        self.bands: Any = None  # band plan of spectrum mode


class AudioVisualizer:
    """
    The audio-visualizer layers of one surface. Every layer's band comes
    from one read of each analysis window per frame.
    """

    def __init__(self, analyzer, rng=None):
        self.analyzer = analyzer
        self.states = {}
        # Drops and fire are random; tests pass a seeded generator
        self.rng = np.random.default_rng() if rng is None else rng
        self._layouts = {}
        self._gradients = {}
        self._plan: Any = ([], None, None)

    @property
    def info(self) -> EffectInfo:
        return INFO

    def prepare(self, layers: Sequence[LayerSpec], colors: tuple[str, ...] = ()) -> None:
        """
        The parts of rendering that only change with the configuration: band
        plans, layouts and colours. `colors` (the album art's) replace the
        palettes.
        """
        parts = []
        params = []
        fast_bands = []
        for index, layer in enumerate(layers):
            seg = layer.params
            if colors:
                seg = dict(seg, palette=cover_palette(seg, colors))
            params.append(seg)
            part = _Part(seg, layer.id, index)
            if part.hits:
                part.fast_index = len(fast_bands)
                fast_bands.append((seg['freq_lo'], seg['freq_hi']))
            part.visible = layer.visible and layer.length > 0
            if part.visible:
                size = part.length = layer.length
                pos, steps = self.layout(size, seg['direction'])
                part.pos, part.steps = pos, steps
                mode = seg['mode']
                if mode in ('fill', 'pulse', 'spectrum'):
                    t = pos / float(max(1, steps - 1))
                    part.colors = self.static_gradient(seg['palette'], size, seg['direction'], t)
                if mode == 'spectrum':
                    edges = np.geomspace(seg['freq_lo'], seg['freq_hi'], part.steps + 1)
                    part.bands = self.analyzer.band_plan(edges, fast=part.hits)
            parts.append(part)
        full = (
            self.analyzer.bands_plan([(s['freq_lo'], s['freq_hi']) for s in params])
            if params
            else None
        )
        fast = self.analyzer.bands_plan(fast_bands, fast=True) if fast_bands else None
        self._plan = (parts, full, fast)

    def layout(self, size, direction):
        """
        Step index of every cell of a layer's line along its direction, and the
        number of steps (mirrored directions have half as many).
        """
        key = (size, direction)
        if key not in self._layouts:
            k = np.arange(size)
            if direction == 'reverse':
                pos = size - 1 - k
            elif direction in ('center', 'edges'):
                pos = np.floor(np.abs(k - (size - 1) / 2.0)).astype(int)
                if direction == 'edges':
                    pos = pos.max() - pos
            else:
                pos = k
            self._layouts[key] = (pos, int(pos.max()) + 1)
        return self._layouts[key]

    @staticmethod
    def gradient(palette, t):
        colors = palette_colors(palette)
        if len(colors) == 1:
            return np.repeat(colors, len(t), axis=0)
        pos = np.clip(t, 0.0, 1.0) * (len(colors) - 1)
        i = np.minimum(pos.astype(int), len(colors) - 2)
        f = (pos - i)[:, None]
        return colors[i] * (1.0 - f) + colors[i + 1] * f

    @staticmethod
    def color_at(palette, t):
        """
        gradient() at a single position, worked out in plain Python up to the
        color itself.
        """
        colors = palette_colors(palette)
        if len(colors) == 1:
            return colors[0].copy()
        pos = (0.0 if t < 0.0 else 1.0 if t > 1.0 else t) * (len(colors) - 1)
        i = min(int(pos), len(colors) - 2)
        f = pos - i
        return colors[i] * (1.0 - f) + colors[i + 1] * f

    def static_gradient(self, palette, size, direction, t):
        """
        gradient() cached by palette, size and direction. `t` may be a
        function returning it, called only when the colors aren't cached.
        """
        key = (tuple(palette), size, direction)
        colors = self._gradients.get(key)
        if colors is None:
            if len(self._gradients) > 256:
                self._gradients.clear()
            colors = self._gradients[key] = self.gradient(palette, t() if callable(t) else t)
        return colors

    def step_gradient(self, palette, steps):
        return self.static_gradient(palette, steps, 'steps', lambda: np.linspace(0.0, 1.0, steps))

    @staticmethod
    def shape(level, seg):
        """
        Maps a level in dB to 0-1: threshold is 0, ceiling is 1. For hits the
        level is dB above the recent average.
        """
        span = seg['ceiling'] - seg['threshold']
        value = np.clip((level - seg['threshold']) / span * seg['gain'], 0.0, 1.0)
        return value ** seg['curve']

    @staticmethod
    def shape_one(level, seg):
        """
        shape() of a single level, in plain Python: numpy takes microseconds
        per call even on one number.
        """
        value = (level - seg['threshold']) / (seg['ceiling'] - seg['threshold']) * seg['gain']
        value = 0.0 if value < 0.0 else 1.0 if value > 1.0 else value
        return value ** seg['curve']

    def _beat_target(self, state, seg, raw, every, beat, dt):
        """
        Beat trigger: the band's held level, shaped like a level trigger,
        for a moment after each beat of the grid (`every` beats), else 0.
        The moment lasts long enough for the attack to bring it up.
        """
        if state.held is None or raw > state.held:
            state.held = raw
        else:
            state.held -= BEAT_HOLD_FALL * dt
        if beat is not None and beat.fired(every):
            state.pulse = max(BEAT_PULSE, 3.0 * seg['attack'] / 1000.0)
        if state.pulse <= 0.0:
            return 0.0
        state.pulse -= dt
        return self.shape_one(state.held, seg)

    @staticmethod
    def _update_peak(state, dt):
        if state.value >= state.peak:
            state.peak = state.value
            state.peak_age = 0.0
        else:
            state.peak_age += dt
            if state.peak_age > PEAK_HOLD:
                state.peak = max(state.value, state.peak - PEAK_FALL * dt)

    def _spectrum(self, state, seg, spectrum, part, dt):
        steps = part.steps
        levels = self.analyzer.planned_levels(spectrum, part.bands)
        if part.hits:
            if state.band_average is None or len(state.band_average) != steps:
                state.band_average = levels.copy()
            state.band_average += (levels - state.band_average) * smoothing(AVERAGE_MS, dt)
            levels = levels - state.band_average
        target = self.shape(levels, seg)
        if state.bands is None or len(state.bands) != steps:
            state.bands = np.zeros(steps)
        rate = np.where(
            target > state.bands, smoothing(seg['attack'], dt), smoothing(seg['release'], dt)
        )
        state.bands += (target - state.bands) * rate
        return state.bands

    @staticmethod
    def _hit(state, target, dt):
        """
        True when the level jumps past HIT_ON after having dropped below
        HIT_OFF, so one hit fires once.
        """
        state.since_hit += dt
        if state.armed and target >= HIT_ON and state.since_hit >= HIT_GAP:
            state.armed = False
            state.since_hit = 0.0
            return True
        if target < HIT_OFF:
            state.armed = True
        return False

    @staticmethod
    def _buffer(state, shape):
        if state.buffer is None or state.buffer.shape != shape:
            state.buffer = np.zeros(shape)
            state.carry = 0.0
        return state.buffer

    @staticmethod
    def _advance(state, speed, dt):
        """
        Whole steps to move this frame at speed steps/second.
        """
        state.carry += speed * dt
        moved = int(state.carry)
        state.carry -= moved
        return moved

    def _scroll(self, state, seg, steps, dt):
        """
        LedFx Scroll: the color of the moment streams away from the start
        and fades out over `width` percent of the line.
        """
        trail = self._buffer(state, (steps, 3))
        moved = min(steps, self._advance(state, seg['speed'], dt))
        if moved:
            trail[moved:] = trail[:-moved]
        length = max(1.0, steps * seg['width'] / 100.0)
        fade = length / max(1.0, seg['speed']) / 3.0  # ~5% left at the end
        trail *= math.exp(-dt / fade)
        color = self.color_at(seg['palette'], state.value) * state.value
        if moved:
            trail[:moved] = color
        else:
            np.maximum(trail[0], color, out=trail[0])
        return np.maximum(trail, self.step_gradient(seg['palette'], steps) * seg['floor'])

    def _drops(self, state, seg, steps, dt, hit, target):
        """
        LedFx Rain and Spotlight: every hit drops a ripple at a random spot.
        It grows at `speed`, is `width` percent wide and fades with release.
        """
        if hit and len(state.drops) < MAX_DROPS:
            color = self.color_at(seg['palette'], float(self.rng.random(1)[0]))
            state.drops.append([self.rng.uniform(0, steps - 1), 0.0, color, target])
        k = np.arange(steps)
        half = max(0.75, steps * seg['width'] / 200.0)
        life = max(20.0, seg['release']) / 1000.0
        rgb = self.step_gradient(seg['palette'], steps) * seg['floor']
        alive = []
        for drop in state.drops:
            center, age, color, strength = drop
            drop[1] = age = age + dt
            radius = seg['speed'] * age
            fade = strength * math.exp(-age / life)
            if fade < 0.02 or radius - half > steps:
                continue
            alive.append(drop)
            ring = np.clip(1.0 - np.abs(np.abs(k - center) - radius) / half, 0.0, 1.0)
            np.maximum(rgb, color * (ring * fade)[:, None], out=rgb)
        state.drops = alive
        return rgb

    def _scan(self, state, seg, steps, dt):
        """
        LedFx Scan: an eye `width` percent long bounces between the ends,
        moving faster the louder the band is.
        """
        length = min(float(steps), max(1.0, steps * seg['width'] / 100.0))
        travel = steps - length
        state.pos += state.heading * seg['speed'] * state.value * dt
        if state.pos >= travel:
            state.pos, state.heading = travel, -1.0
        elif state.pos <= 0.0:
            state.pos, state.heading = 0.0, 1.0
        k = np.arange(steps) + 0.5
        inside = np.minimum(k - state.pos, state.pos + length - k)
        soft = max(0.5, min(2.0, length / 4.0))
        eye = np.clip(inside / soft + 0.5, 0.0, 1.0)
        color = self.color_at(seg['palette'], (state.pos + length / 2.0) / steps)
        level = seg['floor'] + (1.0 - seg['floor']) * state.value
        return color * (eye * level)[:, None]

    def _wipe(self, state, seg, steps, dt, hit):
        """
        LedFx Multicolor Bar and Bar: every hit sweeps the next palette color
        across, turning around each time. Below 100% `width` only a bar that
        long travels.
        """
        palette = seg['palette']
        if hit:
            state.color_index = (state.color_index + 1) % len(palette)
            state.heading = -state.heading
            state.pos = 0.0
        state.pos = min(1.0, max(0.0, state.pos) + seg['speed'] * dt / steps)
        front = (1.0 - (1.0 - state.pos) ** 2) * steps  # eases out like LedFx
        k = np.arange(steps) + 0.5
        s = k if state.heading > 0 else steps - k
        behind = np.clip(front - s + 0.5, 0.0, 1.0)
        colors = palette_colors(palette)
        new = colors[state.color_index % len(palette)]
        if seg['width'] >= 100:
            old = colors[(state.color_index - 1) % len(palette)]
            rgb = new * behind[:, None] + old * (1.0 - behind)[:, None]
        else:
            length = max(1.0, steps * seg['width'] / 100.0)
            rgb = new * (behind * np.clip(s - (front - length) + 0.5, 0.0, 1.0))[:, None]
        return rgb * (seg['floor'] + (1.0 - seg['floor']) * state.value)

    def _flow(self, state, seg, steps, dt):
        """
        LedFx Lava lamp and Marching: palette blobs `width` percent long
        drift along at `speed`, pushed faster and brighter by the band.
        """
        state.pos += seg['speed'] * dt * (0.4 + 2.6 * state.value)
        size = max(2.0, steps * seg['width'] / 100.0)
        k = np.arange(steps, dtype=np.float64)
        p = state.pos
        wave = (
            np.sin(2 * np.pi * (k - p) / size)
            + 0.8 * np.sin(2 * np.pi * (0.73 * k + 0.41 * p) / (1.37 * size) + 1.7)
            + 0.5 * np.sin(2 * np.pi * (k + 0.3 * p) / (2.3 * size) + 4.1)
        )
        blob = np.clip((wave / 2.3 + 1.0) / 2.0, 0.0, 1.0) ** 1.6
        hue = k / steps + p / (3.0 * steps) + 0.25 * blob
        hue = 1.0 - np.abs(np.mod(hue, 2.0) - 1.0)  # back and forth, no seam
        level = seg['floor'] + (1.0 - seg['floor']) * state.value
        return self.gradient(seg['palette'], hue) * (blob * level)[:, None]

    def _fire(self, state, seg, steps, dt):
        """
        LedFx Fire: sparks at the start heat up the line, the heat rises
        at `speed` and cools on the way. The louder the band, the higher the
        flames reach. Colors go from the first (cool) to the last (hot).
        """
        heat = self._buffer(state, (steps,))
        level = seg['floor'] + (1.0 - seg['floor']) * state.value
        state.pos += dt
        moved = min(steps, self._advance(state, seg['speed'], dt))
        if moved:
            heat[moved:] = heat[:-moved]
            heat[:moved] = 0.0
        if steps > 2:
            heat[1:-1] = heat[1:-1] * 0.5 + (heat[:-2] + heat[2:]) * 0.25
        # Cooling, uneven along the line so the flames form tongues
        k = np.arange(steps) / float(steps)
        swirl = np.sin(2 * np.pi * (k / 0.23 - state.pos * 1.3)) * np.sin(
            2 * np.pi * (k / 0.11 + state.pos * 0.7)
        )
        height = max(2.0, steps * (0.2 + 0.8 * level))
        fade = height / max(1.0, seg['speed']) / 1.4  # ~25% left at the tip
        heat *= np.exp(-dt / fade * (1.0 + 0.7 * swirl))
        # Bursts of sparks at the base, more and hotter when it's loud
        if self.rng.random() < 0.3 + 0.5 * level:
            base = max(1, round(steps * 0.06))
            burst = self.rng.uniform(0.6, 1.0) * (0.55 + 0.45 * level)
            spread = burst * self.rng.uniform(0.85, 1.0, base)
            np.maximum(heat[:base], spread, out=heat[:base])
        np.clip(heat, 0.0, 1.0, out=heat)
        bright = np.clip(heat * 1.8, 0.0, 1.0)
        return self.gradient(seg['palette'], heat) * bright[:, None]

    def _motion(self, mode, state, seg, steps, dt, hit, target):
        if mode == 'scroll':
            return self._scroll(state, seg, steps, dt)
        if mode == 'drops':
            return self._drops(state, seg, steps, dt, hit, target)
        if mode == 'scan':
            return self._scan(state, seg, steps, dt)
        if mode == 'wipe':
            return self._wipe(state, seg, steps, dt, hit)
        if mode == 'flow':
            return self._flow(state, seg, steps, dt)
        return self._fire(state, seg, steps, dt)

    def render(
        self, spectrum: Spectrum, dt: float, beat: Any = None
    ) -> tuple[list[Line | None], Levels]:
        """
        Returns each layer's line (None when it isn't visible), in the order
        prepare() got them, and {layer id: (level dB, value, recent average
        dB)}, unrounded. Hits layers react to the short window but report the
        level of the long one, which is what the UI's spectrum shows.
        `beat`: the zone's BeatTracker, pushed with the same audio (beat
        triggers stay dark without one).
        """
        parts, full_plan, fast_plan = self._plan
        lines: list[Line | None] = [None] * len(parts)
        # Every layer's band at once: the long window for all of them (what
        # the UI shows, and what level triggers use), the short one for hits
        full = self.analyzer.planned_levels(spectrum, full_plan).tolist() if full_plan else []
        fast = self.analyzer.planned_levels(spectrum, fast_plan).tolist() if fast_plan else []
        follow = smoothing(AVERAGE_MS, dt)
        states = self.states
        levels = {}

        for part in parts:
            seg = part.seg
            state = states.get(part.id)
            if state is None:
                state = states[part.id] = LayerState()
            shown = full[part.index]
            raw = fast[part.fast_index] if part.hits else shown
            if state.average is None:
                state.average = raw
            state.average += (raw - state.average) * follow
            level = raw - state.average if part.hits else raw
            if part.beats:
                target = self._beat_target(state, seg, raw, part.beats, beat, dt)
            else:
                target = self.shape_one(level, seg)
            ms = seg['attack'] if target > state.value else seg['release']
            state.value += (target - state.value) * smoothing(ms, dt)
            self._update_peak(state, dt)
            hit = self._hit(state, target, dt)
            shown_average = state.average
            if part.hits:
                if state.shown_average is None:
                    state.shown_average = shown
                state.shown_average += (shown - state.shown_average) * follow
                shown_average = state.shown_average
            levels[part.id] = (shown, state.value, shown_average)

            if not part.visible:
                continue
            mode = seg['mode']
            if mode in MOTION_MODES:
                motion = self._motion(mode, state, seg, part.steps, dt, hit, target)
                lines[part.index] = motion[part.pos]
                continue

            floor = seg['floor']
            if mode == 'pulse':
                lines[part.index] = part.colors * (floor + (1.0 - floor) * state.value)
                continue
            if mode == 'shift':
                color = self.color_at(seg['palette'], state.value)
                shifted = color * (floor + (1.0 - floor) * state.value)
                lines[part.index] = np.broadcast_to(shifted, (part.length, 3))
                continue
            pos, steps = part.pos, part.steps
            if mode == 'spectrum':
                bright = self._spectrum(state, seg, spectrum, part, dt)[pos]
            else:
                bright = np.clip(state.value * steps - pos, 0.0, 1.0)
                if seg['peak'] and state.peak > 0.01:
                    bright[pos == min(steps - 1, int(state.peak * steps))] = 1.0
            bright = floor + (1.0 - floor) * bright
            lines[part.index] = part.colors * bright[:, None]

        if len(states) > len(levels):
            for seg_id in [s for s in states if s not in levels]:
                del states[seg_id]

        return lines, levels


def _params_schema() -> Mapping[str, Any]:
    try:
        return clamp.definition('AudioVisualizerParams')
    except OSError:  # a build without the bundled schemas
        return {}


INFO = EffectInfo(
    id=EFFECT_ID,
    version=VERSION,
    name='Audio visualizer',
    description='A frequency band of the audio, shaped and smoothed, in one of ten modes.',
    inputs=('audio', 'beat', 'media'),
    params_schema=_params_schema(),
    min_elements=1,
)
register(EFFECT_ID, AudioVisualizer)
