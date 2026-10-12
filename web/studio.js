'use strict';

/* FOLO studio UI
 *
 * The server owns the audio analysis and the LED output. This page edits the
 * segment config (POST /api/config) and draws the live data streamed from
 * /api/stream.
 */

const DB_MIN = -100;
const DB_MAX = 0;
const HZ_MIN = 10;
const HZ_MAX = 24000;

const PRESETS = {
  'Deep Night': ['#0033CC', '#000080', '#4B00FF', '#FF3399', '#FF00FF', '#800080', '#8F00FF', '#4B0082'],
  'Abyss': ['#0033CC', '#000080', '#4B00FF'],
  'Neon Pink': ['#FF3399', '#FF00FF', '#800080'],
  'Violet': ['#800080', '#8F00FF', '#4B0082'],
  'Synthwave': ['#FF00C8', '#8F00FF', '#00E5FF'],
  'Ocean': ['#001F54', '#0077B6', '#00B4D8', '#90E0EF'],
  'Fire': ['#FF0000', '#FF5A00', '#FFB000', '#FFF1A0'],
  'Rainbow': ['#FF0000', '#FFFF00', '#00FF00', '#00FFFF', '#0000FF', '#FF00FF'],
  'White': ['#FFFFFF'],
};
const NEW_PALETTES = ['Abyss', 'Neon Pink', 'Violet', 'Synthwave', 'Ocean', 'Fire'];

// Reactive presets set how a segment responds, never its colors. `range` is
// the frequency range with thresholds tuned for it (and whether they are
// levels or dB above the recent average), `response` the motion.
// `anim` and `dur` drive the little preview animation.
const RANGE_KEYS = ['freq_lo', 'freq_hi', 'threshold', 'ceiling', 'trigger'];
const RESPONSE_KEYS = ['mode', 'direction', 'attack', 'release', 'gain', 'curve', 'floor', 'peak'];

const rx = (name, tempo, anim, dur, desc, [freq_lo, freq_hi, threshold, ceiling, trigger = 'level'],
  [mode, direction, attack, release, gain, curve, floor, peak = false]) => ({
  name, tempo, anim, dur, desc,
  range: { freq_lo, freq_hi, threshold, ceiling, trigger },
  response: { mode, direction, attack, release, gain, curve, floor, peak },
});

const REACTIVE = [
  ['Low end', [
    rx('Kick punch', 'Fast', 'kick', 0.5, 'Sharp flash on every kick drum.',
      [40, 120, 4, 12, 'hits'], ['pulse', 'center', 0, 90, 1, 1.6, 0]),
    rx('Bass strobe', 'Very fast', 'kick', 0.22, 'Hard on/off flicker on loud bass hits.',
      [40, 100, 6, 12, 'hits'], ['pulse', 'forward', 0, 35, 1, 2, 0]),
    rx('Heartbeat', 'Medium', 'heartbeat', 1.1, 'Instant hit with a long soft fade.',
      [40, 120, -40, -14], ['pulse', 'center', 0, 450, 1, 1.8, 0.04]),
    rx('Sub swell', 'Slow', 'breathe', 3.2, 'Slow rise and fall with the deep sub bass.',
      [20, 60, -50, -18], ['pulse', 'center', 250, 1200, 1, 1, 0.08]),
    rx('Bass meter', 'Fast', 'meter', 0.9, 'Level meter from the middle with a peak marker.',
      [30, 160, -50, -14], ['fill', 'center', 8, 220, 1, 1, 0, true]),
  ]],
  ['Mids', [
    rx('Snare snap', 'Fast', 'kick', 0.45, 'Quick bright hit on snares and claps.',
      [1800, 5000, 5, 14, 'hits'], ['pulse', 'center', 0, 90, 1, 1.6, 0]),
    rx('Vocal glow', 'Slow', 'breathe', 2.4, 'Soft glow that follows voices and leads.',
      [300, 3000, -48, -22], ['pulse', 'forward', 150, 900, 1, 1.2, 0.1]),
    rx('Mid ripple', 'Medium', 'eq', 0.8, 'Each LED dances to its own slice of the mids.',
      [200, 2000, -55, -28], ['spectrum', 'center', 15, 250, 1, 1, 0]),
    rx('Guitar wave', 'Medium', 'meter', 1.4, 'Smooth fill that rolls along the segment.',
      [250, 1500, -50, -25], ['fill', 'forward', 40, 500, 1, 1.3, 0.05]),
    rx('Slow breathe', 'Slow', 'breathe', 4.5, 'Very slow breathing with the overall energy.',
      [100, 2000, -50, -25], ['pulse', 'center', 600, 2500, 1.2, 1, 0.15]),
  ]],
  ['Highs', [
    rx('Hi-hat sparkle', 'Fast', 'eq', 0.35, 'Crisp sparkles on hi-hats and percussion.',
      [6000, 16000, 5, 15, 'hits'], ['spectrum', 'forward', 0, 120, 1, 1.4, 0]),
    rx('Cymbal flash', 'Very fast', 'kick', 0.2, 'Short white-hot flashes on cymbals.',
      [8000, 18000, 7, 18, 'hits'], ['pulse', 'center', 0, 50, 1, 1.8, 0]),
    rx('Air twinkle', 'Medium', 'eq', 0.9, 'Twinkling from both ends on the airy top end.',
      [10000, 20000, -75, -50], ['spectrum', 'edges', 0, 250, 1.2, 1.4, 0]),
    rx('Shimmer', 'Slow', 'breathe', 2, 'Gentle shimmer that swells with the treble.',
      [4000, 16000, -62, -38], ['pulse', 'forward', 80, 700, 1, 1, 0.06]),
  ]],
  ['Full range', [
    rx('Beat flash', 'Very fast', 'kick', 0.3, 'Flashes on the loudest moments of the track.',
      [30, 16000, 4, 12, 'hits'], ['pulse', 'forward', 0, 70, 1, 1.8, 0]),
    rx('VU meter', 'Fast', 'meter', 0.7, 'Classic volume meter with a peak marker.',
      [20, 16000, -45, -10], ['fill', 'forward', 5, 300, 1, 1, 0, true]),
    rx('Spectrum bars', 'Fast', 'eq', 0.6, 'The whole spectrum spread along the segment.',
      [30, 16000, -70, -25], ['spectrum', 'forward', 5, 200, 1, 1, 0]),
    rx('Mirror spectrum', 'Fast', 'eq', 0.6, 'Spectrum from the middle out, bass in the center.',
      [30, 12000, -70, -25], ['spectrum', 'center', 5, 180, 1, 1, 0]),
    rx('Color drift', 'Slow', 'drift', 6, 'Colors drift through the palette with the energy.',
      [40, 4000, -45, -15], ['shift', 'forward', 400, 1500, 1, 1, 0.45]),
    rx('Calm ambient', 'Slow', 'breathe', 6, 'Barely moving glow for background listening.',
      [20, 8000, -50, -15], ['pulse', 'center', 1200, 4000, 1, 1, 0.35]),
  ]],
];

let reactiveScope = 'selected';
let reactiveTiles = [];

const BANDS = [
  ['Sub', 20, 60], ['Bass', 60, 250], ['Low mid', 250, 500], ['Mid', 500, 2000],
  ['High mid', 2000, 4000], ['Presence', 4000, 6000], ['Air', 6000, 20000],
];
const MODES = [
  ['fill', 'Fill'], ['pulse', 'Pulse'], ['spectrum', 'Spectrum'], ['shift', 'Color shift'],
  // Moving modes, after LedFx effects
  ['scroll', 'Scroll'], ['drops', 'Drops'], ['scan', 'Scan'], ['wipe', 'Wipe'], ['flow', 'Flow'], ['fire', 'Fire'],
];
const MODE_HELP = {
  fill: 'Lights up along the segment like a level meter.',
  pulse: 'The whole segment brightens with the level.',
  spectrum: 'Each LED follows its own slice of the frequency range.',
  shift: 'The level picks the color from the palette.',
  scroll: 'LedFx Scroll: the color of the moment streams away from the start and fades out along the trail.',
  drops: 'LedFx Rain and Spotlight: every hit drops a ripple at a random spot. Speed 0 keeps it a still spot; release sets how long it lasts.',
  scan: 'LedFx Scan: an eye bounces between the ends, moving faster the louder it gets. Idle glow keeps it visible when quiet.',
  wipe: 'LedFx Multicolor Bar: every hit sweeps the next palette color across, turning around each time. Under 100% size a bar travels instead.',
  flow: 'LedFx Lava lamp: palette blobs drift along on their own, pushed faster and brighter by the music.',
  fire: 'LedFx Fire: flames rise from the start, reaching higher the louder it gets. Colors go from cool (first) to hot (last).',
};
// Speed (LEDs per second) and size (% of the segment) only matter for moving
// modes; the size field is named after what it sets
const MOTION_DEFAULTS = { speed: 120, width: 20 };
const SIZE_LABEL = { scroll: 'Trail', drops: 'Drop size', scan: 'Eye size', wipe: 'Bar size', flow: 'Blob size' };
const SPEED_MODES = new Set(['scroll', 'drops', 'scan', 'wipe', 'flow', 'fire']);
const TRIGGERS = [
  ['level', 'Level', 'Lights when the band is louder than the threshold'],
  ['hits', 'Hits', 'Lights on sudden peaks above the band’s recent average'],
];
const TRIGGER_HELP = {
  level: 'Lights when the band is louder than the threshold. Best for meters and steady glows.',
  hits: 'Lights on sudden peaks above the band’s average over the last second, so it adapts to every song and volume. Best for fast flicker.',
};
const DIRECTIONS = [
  ['forward', '→ Forward', 'From the first LED to the last'],
  ['reverse', '← Reverse', 'From the last LED to the first'],
  ['center', '↔ Center', 'From the middle outwards'],
  ['edges', '→← Edges', 'From both ends to the middle'],
];

let config = null;
let devices = [];
let defaultDevice = '';
let apps = [];                  // apps with audio on an output right now (see /api/sources)
let display = { bands: 192, min_hz: 20, max_hz: 20000 };
let centers = [];
let selectedId = null;
let live = { spectrum: null, leds: '', levels: {}, status: null };
let ledBytes = new Uint8Array(0);
let online = false;
let dragging = null;
let saveTimer = null;
let pendingSave = false;
let saving = 0;
let editSeq = 0;
let syncers = [];
let meterRefs = null;
const listRows = new Map();
const spec = { smooth: null, peaks: null };
// The canvases and meters only change with new stream data or user input, so
// a frame is drawn only after one of those (at most one per display frame)
// instead of on every display refresh
let frameRequested = false;
function invalidate() {
  if (frameRequested) return;
  frameRequested = true;
  requestAnimationFrame(frame);
}

/* ---------- helpers ---------- */

const $ = (sel) => document.querySelector(sel);
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

function h(tag, attrs, children) {
  const node = document.createElement(tag);
  if (attrs) {
    for (const [key, value] of Object.entries(attrs)) {
      if (value === null || value === undefined || value === false) continue;
      if (key === 'class') node.className = value;
      else node.setAttribute(key, value === true ? '' : value);
    }
  }
  if (children !== undefined && children !== null) {
    for (const child of [].concat(children)) {
      if (child === null || child === undefined || child === false) continue;
      node.append(child instanceof Node ? child : String(child));
    }
  }
  return node;
}

function hexToRgb(hex) {
  return [parseInt(hex.slice(1, 3), 16), parseInt(hex.slice(3, 5), 16), parseInt(hex.slice(5, 7), 16)];
}

// A readable accent color for a palette on the dark background
function accentOf(palette) {
  let best = hexToRgb(palette[0]);
  let score = -1;
  for (const color of palette) {
    const rgb = hexToRgb(color);
    const s = Math.max(...rgb) * 2 + rgb[0] + rgb[1] + rgb[2];
    if (s > score) { score = s; best = rgb; }
  }
  const lum = (0.2126 * best[0] + 0.7152 * best[1] + 0.0722 * best[2]) / 255;
  const mix = lum < 0.3 ? 0.38 : 0.1;
  return best.map((c) => Math.round(c + (255 - c) * mix));
}

const rgba = (rgb, a) => `rgba(${rgb[0]},${rgb[1]},${rgb[2]},${a})`;
const gradientCss = (p) => `linear-gradient(90deg, ${(p.length > 1 ? p : [p[0], p[0]]).join(', ')})`;

function fmtHz(f) {
  if (f >= 1000) {
    const k = f / 1000;
    return `${k >= 10 ? Math.round(k) : Number(k.toFixed(1))} kHz`;
  }
  return `${Math.round(f)} Hz`;
}

const roundHz = (f) => Number(f.toPrecision(3));
const roundDb = (d) => Math.round(d * 2) / 2;

function newId() {
  if (window.crypto && crypto.randomUUID) return crypto.randomUUID().replace(/-/g, '').slice(0, 8);
  return Math.random().toString(16).slice(2, 10);
}

function hexToBytes(hex) {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

function rrect(ctx, x, y, w, hgt, r) {
  r = Math.max(0, Math.min(r, w / 2, hgt / 2));
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + hgt, r);
  ctx.arcTo(x + w, y + hgt, x, y + hgt, r);
  ctx.arcTo(x, y + hgt, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function fitCanvas(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const hgt = canvas.clientHeight;
  const pw = Math.round(w * dpr);
  const ph = Math.round(hgt * dpr);
  if (canvas.width !== pw || canvas.height !== ph) {
    canvas.width = pw;
    canvas.height = ph;
  }
  const ctx = canvas.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h: hgt };
}

// A static layer (the analyzer's grid, the strip's LED shapes) is drawn once
// into an offscreen canvas the size of `canvas`, and again only when that size
// or `key` changes. Copy it in with drawImage(layer, 0, 0, w, h).
function cachedLayer(slot, canvas, key, draw) {
  const dpr = window.devicePixelRatio || 1;
  const fullKey = `${canvas.width}x${canvas.height}@${dpr}|${key}`;
  if (slot.key !== fullKey) {
    slot.key = fullKey;
    slot.canvas = slot.canvas || document.createElement('canvas');
    slot.canvas.width = canvas.width;
    slot.canvas.height = canvas.height;
    const ctx = slot.canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw(ctx, canvas.width / dpr, canvas.height / dpr);
  }
  return slot.canvas;
}

function localPos(e, canvas) {
  const rect = canvas.getBoundingClientRect();
  return { x: e.clientX - rect.left, y: e.clientY - rect.top };
}

let toastTimer = null;
// action: optional { label, run } shown as a button (e.g. Undo)
function toast(message, action = null) {
  const el = $('#toast');
  el.replaceChildren(h('span', null, message));
  if (action) {
    const button = h('button', { type: 'button', class: 'toast-action' }, action.label);
    button.addEventListener('click', () => {
      el.classList.remove('show');
      action.run();
    });
    el.append(button);
  }
  el.classList.toggle('has-action', !!action);
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), action ? 9000 : 3200);
}

// Buttons for destructive actions ask for a second click
// Both labels share one grid cell, so the button never changes size
function confirmClick(button, label, action, needsConfirm = () => true) {
  button.replaceChildren(h('span', null, button.textContent), h('span', { class: 'ask' }, label));
  button.classList.add('two-label');
  let timer = null;
  const disarm = () => {
    clearTimeout(timer);
    button.classList.remove('confirm');
  };
  button.addEventListener('click', () => {
    if (!needsConfirm()) {
      action();
      return;
    }
    if (button.classList.contains('confirm')) {
      disarm();
      action();
      return;
    }
    button.classList.add('confirm');
    timer = setTimeout(disarm, 3000);
  });
}

/* ---------- config ---------- */

const selected = () => (config ? config.segments.find((s) => s.id === selectedId) || null : null);
const segById = (id) => config.segments.find((s) => s.id === id) || null;
const soloActive = () => config.segments.some((s) => s.solo && s.enabled);

async function api(path, body) {
  const options = body === undefined ? {} : {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  };
  const res = await fetch(path, options);
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

function commit() {
  editSeq++;
  pendingSave = true;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    pendingSave = false;
    const sent = editSeq;
    saving++;
    try {
      const saved = await api('/api/config', config);
      // Take the server's cleaned up values unless the user kept editing
      if (sent === editSeq && !dragging) {
        config = saved;
        if (!selected()) selectedId = null;
        refreshValues();
      }
    } catch (err) {
      toast(`Could not save: ${err.message}`);
    } finally {
      saving--;
    }
  }, 60);
}

// Keeps a segment valid after `key` changed, pushing the related value
function enforce(seg, key) {
  const n = config.led_count;
  seg.start = clamp(Math.round(seg.start), 0, n - 1);
  seg.end = clamp(Math.round(seg.end), 0, n - 1);
  if (seg.start > seg.end) {
    if (key === 'start') seg.end = seg.start;
    else seg.start = seg.end;
  }
  seg.freq_lo = clamp(seg.freq_lo, HZ_MIN, HZ_MAX / 1.02);
  seg.freq_hi = clamp(seg.freq_hi, HZ_MIN * 1.02, HZ_MAX);
  if (seg.freq_hi < seg.freq_lo * 1.02) {
    if (key === 'freq_hi') seg.freq_lo = seg.freq_hi / 1.02;
    else seg.freq_hi = seg.freq_lo * 1.02;
  }
  const limits = dbLimits(seg);
  seg.threshold = clamp(seg.threshold, limits.threshold[0], limits.threshold[1]);
  seg.ceiling = clamp(seg.ceiling, limits.ceiling[0], limits.ceiling[1]);
  if (seg.ceiling < seg.threshold + 1) {
    if (key === 'ceiling') seg.threshold = seg.ceiling - 1;
    else seg.ceiling = seg.threshold + 1;
  }
}

// Level thresholds are dB levels; hits thresholds are dB above the band's
// recent average (same limits as the server)
const HITS_MAX_DB = 40;
function dbLimits(seg) {
  return seg.trigger === 'hits'
    ? { threshold: [0, HITS_MAX_DB - 1], ceiling: [1, HITS_MAX_DB] }
    : { threshold: [-120, -1], ceiling: [-119, 0] };
}

// The band's recent average from the engine, which hits are measured from
function recentAverage(seg) {
  const level = live.levels && live.levels[seg.id];
  return level && level.length > 2 ? level[2] : -60;
}

// dB on the analyzer scale where a segment's threshold or ceiling sits
const dbBase = (seg) => (seg.trigger === 'hits' ? recentAverage(seg) : 0);

function setValue(key, value) {
  const seg = selected();
  if (!seg) return;
  if (key === 'trigger' && value !== seg.trigger) {
    // Keep the box where it is right now when switching
    const base = recentAverage(seg);
    if (value === 'hits') {
      seg.threshold = 4;
      seg.ceiling = 12;
    } else {
      seg.threshold = roundDb(clamp(base + seg.threshold, -120, -1));
      seg.ceiling = roundDb(base + seg.ceiling);
    }
    seg.trigger = value;
    enforce(seg, null);
    renderInspector();
    updateListRow(seg);
    commit();
    return;
  }
  seg[key] = value;
  enforce(seg, key);
  changed(seg);
}

function changed(seg) {
  syncInspector();
  updateListRow(seg);
  commit();
}

function bandName(lo, hi) {
  const center = Math.sqrt(lo * hi);
  const band = BANDS.find(([, a, b]) => center >= a && center < b);
  return band ? band[0] : 'Segment';
}

function uniqueName(base) {
  const names = new Set(config.segments.map((s) => s.name));
  if (!names.has(base)) return base;
  for (let i = 2; ; i++) if (!names.has(`${base} ${i}`)) return `${base} ${i}`;
}

// Longest run of LEDs not used by any segment, or the whole strip
function freeLedRange() {
  const n = config.led_count;
  const used = new Uint8Array(n);
  for (const s of config.segments) used.fill(1, s.start, s.end + 1);
  let best = null;
  let start = -1;
  for (let i = 0; i <= n; i++) {
    if (i < n && !used[i]) {
      if (start < 0) start = i;
    } else if (start >= 0) {
      if (!best || i - start > best[1] - best[0] + 1) best = [start, i - 1];
      start = -1;
    }
  }
  return best && best[1] - best[0] >= 1 ? best : [0, n - 1];
}

function addSegment(partial = {}) {
  const count = config.segments.length;
  const band = BANDS[count % BANDS.length];
  const range = freeLedRange();
  const seg = Object.assign({
    id: newId(),
    enabled: true,
    solo: false,
    start: range[0],
    end: range[1],
    freq_lo: band[1],
    freq_hi: band[2],
    trigger: 'level',
    threshold: -60,
    ceiling: -25,
    gain: 1,
    curve: 1,
    attack: 15,
    release: 200,
    floor: 0,
    mode: 'fill',
    direction: 'forward',
    peak: false,
    ...MOTION_DEFAULTS,
    palette: [...PRESETS[NEW_PALETTES[count % NEW_PALETTES.length]]],
  }, partial);
  seg.name = partial.name || uniqueName(bandName(seg.freq_lo, seg.freq_hi));
  enforce(seg, null);
  config.segments.push(seg);
  selectedId = seg.id;
  renderList();
  renderInspector();
  commit();
}

function duplicateSelected() {
  const seg = selected();
  if (!seg) return;
  const copy = JSON.parse(JSON.stringify(seg));
  copy.id = newId();
  copy.name = uniqueName(`${seg.name} copy`);
  copy.solo = false;
  config.segments.push(copy);
  selectedId = copy.id;
  renderList();
  renderInspector();
  commit();
}

// Toast with an Undo that brings back the segments as they were before
function undoToast(message) {
  const before = { segments: JSON.parse(JSON.stringify(config.segments)), selected: selectedId };
  return () => toast(message, {
    label: 'Undo',
    run: () => {
      config.segments = before.segments;
      selectedId = before.selected;
      renderList();
      renderInspector();
      commit();
      toast('Undone');
    },
  });
}

function deleteSelected() {
  const index = config.segments.findIndex((s) => s.id === selectedId);
  if (index < 0) return;
  const done = undoToast(`Deleted ${config.segments[index].name}`);
  config.segments.splice(index, 1);
  const next = config.segments[Math.min(index, config.segments.length - 1)];
  selectedId = next ? next.id : null;
  renderList();
  renderInspector();
  commit();
  done();
}

// Lows, mids or highs by name ("Lows 2"), else by the middle of the band
const KINDS = ['Lows', 'Mids', 'Highs'];
function kindOf(seg) {
  const named = KINDS.find((k) => seg.name === k || seg.name.startsWith(`${k} `));
  if (named) return named;
  const center = Math.sqrt(seg.freq_lo * seg.freq_hi);
  return center < 250 ? 'Lows' : center < 4000 ? 'Mids' : 'Highs';
}

// Settings for a new segment of a kind when the strip has none to copy
function kindDefaults(kind) {
  const common = { trigger: 'hits', gain: 1, floor: 0, mode: 'pulse', peak: false, ...MOTION_DEFAULTS };
  if (kind === 'Lows') return { ...common, freq_lo: 35, freq_hi: 150, threshold: 2, ceiling: 10, curve: 1.6, attack: 0, release: 45, direction: 'center', palette: [...LOWS_PAL] };
  if (kind === 'Mids') return { ...common, freq_lo: 250, freq_hi: 2000, threshold: 1, ceiling: 9, curve: 1.2, attack: 25, release: 300, direction: 'center', palette: [...MIDS_PAL] };
  return { ...common, freq_lo: 5000, freq_hi: 16000, threshold: 4, ceiling: 13, curve: 1.3, attack: 0, release: 40, direction: 'forward', palette: [...HIGHS_PAL] };
}

// Adds a lows, mids or highs segment that reacts like the others of its kind.
// It goes into free space, or takes the right half of the selected segment
// (the widest one when none is selected).
function addKind(kind) {
  const n = config.led_count;
  const siblings = config.segments.filter((s) => kindOf(s) === kind);
  const template = siblings.length ? siblings[siblings.length - 1] : null;
  // The lowest free number, so deleting Mids 2 and adding mids gives Mids 2 again
  const numbers = new Set(siblings.map((s) => Number((s.name.match(/ (\d+)$/) || [])[1]) || 1));
  let number = 1;
  while (numbers.has(number)) number++;
  const name = uniqueName(`${kind} ${number}`);

  let [start, end] = freeLedRange();
  const free = end - start + 1 >= 4 && !config.segments.some((s) => s.start <= end && s.end >= start);
  let host = null;
  if (!free) {
    host = selected() || config.segments.reduce((a, b) => (b.end - b.start > a.end - a.start ? b : a), config.segments[0]);
    if (!host || host.end - host.start < 1) {
      toast('No room on the strip: delete or shrink a segment first');
      return;
    }
    start = host.start + Math.ceil((host.end - host.start + 1) / 2);
    end = host.end;
  }
  const done = undoToast(host
    ? `Added ${name} on LEDs ${start}–${end}, the right half of ${host.name}`
    : `Added ${name} on LEDs ${start}–${end}`);
  if (host) host.end = start - 1;

  const settings = template ? JSON.parse(JSON.stringify(template)) : kindDefaults(kind);
  addSegment({ ...settings, id: newId(), name, enabled: true, solo: false, start: Math.min(start, n - 1), end, lane: host ? host.lane : -1 });
  // List it in strip order rather than last
  const added = config.segments.pop();
  const index = config.segments.findIndex((s) => s.start > added.start);
  config.segments.splice(index < 0 ? config.segments.length : index, 0, added);
  renderList();
  done();
}

function autoSplit(n) {
  const leds = config.led_count;
  n = clamp(Math.round(n) || 6, 1, Math.min(16, leds));
  const lo = 30;
  const hi = 16000;
  const deep = PRESETS['Deep Night'];
  const segments = [];
  for (let i = 0; i < n; i++) {
    const f1 = lo * Math.pow(hi / lo, i / n);
    const f2 = lo * Math.pow(hi / lo, (i + 1) / n);
    const c = Math.floor((i * deep.length) / n);
    const threshold = -50 - 22 * (n > 1 ? i / (n - 1) : 0);
    segments.push({
      id: newId(),
      name: `${i + 1} · ${bandName(f1, f2)}`,
      enabled: true,
      solo: false,
      start: Math.round((i * leds) / n),
      end: Math.round(((i + 1) * leds) / n) - 1,
      freq_lo: roundHz(f1),
      freq_hi: roundHz(f2),
      trigger: 'level',
      threshold: roundDb(threshold),
      ceiling: roundDb(threshold + 32),
      gain: 1,
      curve: 1,
      attack: 10,
      release: 220,
      floor: 0,
      mode: 'pulse',
      direction: 'center',
      peak: false,
      ...MOTION_DEFAULTS,
      palette: [deep[c % deep.length], deep[(c + 1) % deep.length]],
    });
  }
  config.segments = segments;
  selectedId = segments[0].id;
  renderList();
  renderInspector();
  commit();
}

/* ---------- spectrum analyzer ---------- */

const anCanvas = $('#analyzer');
const AN_PAD = { l: 42, r: 12, t: 14, b: 24 };
let ag = { x0: AN_PAD.l, y0: AN_PAD.t, w: 1, h: 1 };

const fx = (f) => ag.x0 + (Math.log(f / display.min_hz) / Math.log(display.max_hz / display.min_hz)) * ag.w;
const xf = (x) => display.min_hz * Math.pow(display.max_hz / display.min_hz, (x - ag.x0) / ag.w);
const dy = (d) => ag.y0 + ((DB_MAX - d) / (DB_MAX - DB_MIN)) * ag.h;
const yd = (y) => DB_MAX - ((y - ag.y0) / ag.h) * (DB_MAX - DB_MIN);

function ingestSpectrum(values) {
  if (!values) return;
  if (!spec.smooth || spec.smooth.length !== values.length) {
    spec.smooth = Float32Array.from(values);
    spec.peaks = Float32Array.from(values);
    return;
  }
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    const s = spec.smooth[i];
    spec.smooth[i] = v > s ? v : s + (v - s) * 0.35;
    spec.peaks[i] = Math.max(v, spec.peaks[i] - 0.6);
  }
}

// A hits box rides on the band's recent average
function boxOf(seg) {
  const base = dbBase(seg);
  return { x0: fx(seg.freq_lo), x1: fx(seg.freq_hi), yT: dy(base + seg.ceiling), yB: dy(base + seg.threshold) };
}

const anGrid = {};

function drawAnalyzerGrid(ctx) {
  ctx.font = '11px ui-monospace, Consolas, monospace';
  ctx.lineWidth = 1;
  ctx.textBaseline = 'middle';
  ctx.textAlign = 'right';
  for (let d = DB_MIN; d <= DB_MAX; d += 10) {
    const y = Math.round(dy(d)) + 0.5;
    ctx.strokeStyle = d === DB_MIN ? 'rgba(255,255,255,0.14)' : 'rgba(255,255,255,0.05)';
    ctx.beginPath();
    ctx.moveTo(ag.x0, y);
    ctx.lineTo(ag.x0 + ag.w, y);
    ctx.stroke();
    if (d % 20 === 0) {
      ctx.fillStyle = '#5f5d78';
      ctx.fillText(String(d), ag.x0 - 6, y);
    }
  }
  ctx.textAlign = 'center';
  ctx.textBaseline = 'top';
  let lastLabel = -Infinity;
  for (const f of [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]) {
    const x = Math.round(fx(f)) + 0.5;
    ctx.strokeStyle = 'rgba(255,255,255,0.05)';
    ctx.beginPath();
    ctx.moveTo(x, ag.y0);
    ctx.lineTo(x, ag.y0 + ag.h);
    ctx.stroke();
    if (x - lastLabel > 34) {
      ctx.fillStyle = '#5f5d78';
      ctx.fillText(f >= 1000 ? `${f / 1000}k` : String(f), clamp(x, ag.x0 + 8, ag.x0 + ag.w - 12), ag.y0 + ag.h + 6);
      lastLabel = x;
    }
  }
}

function drawAnalyzer() {
  const { ctx, w, h: hgt } = fitCanvas(anCanvas);
  ag = { x0: AN_PAD.l, y0: AN_PAD.t, w: Math.max(1, w - AN_PAD.l - AN_PAD.r), h: Math.max(1, hgt - AN_PAD.t - AN_PAD.b) };
  ctx.clearRect(0, 0, w, hgt);
  ctx.drawImage(cachedLayer(anGrid, anCanvas, `${display.min_hz}-${display.max_hz}`, drawAnalyzerGrid), 0, 0, w, hgt);

  ctx.save();
  ctx.beginPath();
  ctx.rect(ag.x0, ag.y0, ag.w, ag.h);
  ctx.clip();

  // Spectrum
  if (spec.smooth) {
    const bottom = ag.y0 + ag.h;
    const fill = ctx.createLinearGradient(0, ag.y0, 0, bottom);
    fill.addColorStop(0, 'rgba(255,79,168,0.55)');
    fill.addColorStop(0.55, 'rgba(143,0,255,0.28)');
    fill.addColorStop(1, 'rgba(0,51,204,0.05)');
    ctx.beginPath();
    ctx.moveTo(fx(centers[0]), bottom);
    for (let i = 0; i < spec.smooth.length; i++) ctx.lineTo(fx(centers[i]), dy(clamp(spec.smooth[i], DB_MIN, DB_MAX)));
    ctx.lineTo(fx(centers[centers.length - 1]), bottom);
    ctx.closePath();
    ctx.fillStyle = fill;
    ctx.fill();

    ctx.beginPath();
    for (let i = 0; i < spec.smooth.length; i++) {
      const x = fx(centers[i]);
      const y = dy(clamp(spec.smooth[i], DB_MIN, DB_MAX));
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = 'rgba(255,255,255,0.8)';
    ctx.lineWidth = 1.25;
    ctx.stroke();

    ctx.beginPath();
    for (let i = 0; i < spec.peaks.length; i++) {
      const x = fx(centers[i]);
      const y = dy(clamp(spec.peaks[i], DB_MIN, DB_MAX));
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = 'rgba(255,255,255,0.22)';
    ctx.lineWidth = 1;
    ctx.stroke();
  }

  // Segment boxes, selected on top
  const solo = soloActive();
  const ordered = config.segments.filter((s) => s.id !== selectedId);
  const sel = selected();
  if (sel) ordered.push(sel);
  for (const seg of ordered) drawSegmentBox(ctx, seg, seg === sel, !seg.enabled || (solo && !seg.solo));

  // Box being drawn
  if (dragging && dragging.area === 'an' && dragging.part === 'create') {
    const x = Math.min(dragging.sx, dragging.x);
    const y = Math.min(dragging.sy, dragging.y);
    ctx.setLineDash([5, 4]);
    ctx.strokeStyle = 'rgba(255,255,255,0.7)';
    ctx.fillStyle = 'rgba(255,255,255,0.06)';
    ctx.fillRect(x, y, Math.abs(dragging.x - dragging.sx), Math.abs(dragging.y - dragging.sy));
    ctx.strokeRect(x + 0.5, y + 0.5, Math.abs(dragging.x - dragging.sx), Math.abs(dragging.y - dragging.sy));
    ctx.setLineDash([]);
  }
  ctx.restore();
}

function drawSegmentBox(ctx, seg, isSelected, dim) {
  const { x0, x1, yT, yB } = boxOf(seg);
  const accent = accentOf(seg.palette);
  const level = live.levels && live.levels[seg.id];
  const value = level ? level[1] : 0;
  const bottom = ag.y0 + ag.h;
  const strength = dim ? 0.35 : 1;

  // Frequency column and threshold to ceiling box
  ctx.fillStyle = rgba(accent, 0.035 * strength);
  ctx.fillRect(x0, yB, x1 - x0, bottom - yB);
  ctx.fillStyle = rgba(accent, (0.07 + value * 0.28 + (isSelected ? 0.05 : 0)) * strength);
  ctx.fillRect(x0, yT, x1 - x0, yB - yT);

  ctx.strokeStyle = rgba(accent, (isSelected ? 0.9 : 0.45) * strength);
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(x0 + 0.5, yT);
  ctx.lineTo(x0 + 0.5, bottom);
  ctx.moveTo(x1 - 0.5, yT);
  ctx.lineTo(x1 - 0.5, bottom);
  ctx.stroke();

  // Threshold (solid) and ceiling (dashed)
  ctx.strokeStyle = rgba(accent, (isSelected ? 1 : 0.75) * strength);
  ctx.lineWidth = isSelected ? 2.5 : 1.5;
  ctx.beginPath();
  ctx.moveTo(x0, yB);
  ctx.lineTo(x1, yB);
  ctx.stroke();
  ctx.setLineDash([5, 4]);
  ctx.lineWidth = isSelected ? 1.75 : 1.25;
  ctx.beginPath();
  ctx.moveTo(x0, yT);
  ctx.lineTo(x1, yT);
  ctx.stroke();
  ctx.setLineDash([]);

  // Current level of the band
  if (level && !dim) {
    const y = dy(clamp(level[0], DB_MIN, DB_MAX));
    ctx.strokeStyle = value > 0 ? 'rgba(255,255,255,0.95)' : 'rgba(255,255,255,0.4)';
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x0 + 2, y);
    ctx.lineTo(x1 - 2, y);
    ctx.stroke();
  }

  // Handles
  if (isSelected) {
    ctx.fillStyle = '#fff';
    const midY = (yT + yB) / 2;
    const midX = (x0 + x1) / 2;
    for (const [x, y] of [[x0, midY], [x1, midY], [midX, yB], [midX, yT]]) {
      ctx.beginPath();
      ctx.arc(x, y, 3.5, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  // Label
  if (x1 - x0 > 26) {
    ctx.font = `${isSelected ? 600 : 500} 11px system-ui, sans-serif`;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';
    ctx.fillStyle = rgba(accent, strength);
    const y = yT - ag.y0 > 16 ? yT - 6 : yT + 14;
    ctx.fillText(seg.name, x0 + 5, y, x1 - x0 - 8);
  }
}

function hitAnalyzer(x, y) {
  const T = 6;
  const others = config.segments.filter((s) => s.id !== selectedId).reverse();
  const order = selected() ? [selected(), ...others] : others;
  for (const seg of order) {
    const { x0, x1, yT, yB } = boxOf(seg);
    if (x < x0 - T || x > x1 + T || y < yT - T || y > yB + T) continue;
    if (Math.abs(y - yB) <= T && x > x0 && x < x1) return { seg, part: 'threshold' };
    if (Math.abs(y - yT) <= T && x > x0 && x < x1) return { seg, part: 'ceiling' };
    if (Math.abs(x - x0) <= T) return { seg, part: 'freq_lo' };
    if (Math.abs(x - x1) <= T) return { seg, part: 'freq_hi' };
    if (x > x0 && x < x1 && y > yT && y < yB) return { seg, part: 'move' };
  }
  return null;
}

const CURSORS = { freq_lo: 'ew-resize', freq_hi: 'ew-resize', threshold: 'ns-resize', ceiling: 'ns-resize', move: 'grab' };
const PART_LABELS = { freq_lo: 'low edge', freq_hi: 'high edge', threshold: 'threshold', ceiling: 'full brightness', move: 'move' };

// Readouts are overlays, shown on the side of the chart away from the pointer
function setReadout(el, text, x, width) {
  if (el.textContent !== text) el.textContent = text;
  el.classList.toggle('flip', x > width * 0.55);
}

function analyzerReadout(x, y, hit) {
  const el = $('#an-readout');
  if (x < ag.x0 || x > ag.x0 + ag.w || y < ag.y0 || y > ag.y0 + ag.h) {
    el.textContent = '';
    return;
  }
  const text = `${fmtHz(xf(x))} · ${Math.round(yd(y))} dB`;
  setReadout(el, hit ? `${text} · ${hit.seg.name} ${PART_LABELS[hit.part]}` : text, x, anCanvas.clientWidth);
}

function setupAnalyzer() {
  anCanvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    const { x, y } = localPos(e, anCanvas);
    const hit = hitAnalyzer(x, y);
    anCanvas.setPointerCapture(e.pointerId);
    if (hit) {
      select(hit.seg.id);
      dragging = { area: 'an', part: hit.part, id: hit.seg.id, sx: x, sy: y, orig: { ...hit.seg }, base: dbBase(hit.seg) };
      if (hit.part === 'move') anCanvas.style.cursor = 'grabbing';
    } else {
      dragging = { area: 'an', part: 'create', sx: x, sy: y, x, y };
    }
  });

  anCanvas.addEventListener('pointermove', (e) => {
    const { x, y } = localPos(e, anCanvas);
    if (!dragging || dragging.area !== 'an') {
      const hit = hitAnalyzer(x, y);
      anCanvas.style.cursor = hit ? CURSORS[hit.part] : 'crosshair';
      analyzerReadout(x, y, hit);
      return;
    }
    analyzerReadout(x, y, null);
    const d = dragging;
    if (d.part === 'create') {
      d.x = clamp(x, ag.x0, ag.x0 + ag.w);
      d.y = clamp(y, ag.y0, ag.y0 + ag.h);
      return;
    }
    const seg = segById(d.id);
    if (!seg) return;
    const o = d.orig;
    if (d.part === 'freq_lo') {
      seg.freq_lo = roundHz(clamp(xf(x), HZ_MIN, seg.freq_hi / 1.02));
    } else if (d.part === 'freq_hi') {
      seg.freq_hi = roundHz(clamp(xf(x), seg.freq_lo * 1.02, HZ_MAX));
    } else if (d.part === 'threshold') {
      seg.threshold = roundDb(clamp(yd(y) - d.base, dbLimits(seg).threshold[0], seg.ceiling - 1));
    } else if (d.part === 'ceiling') {
      seg.ceiling = roundDb(clamp(yd(y) - d.base, seg.threshold + 1, dbLimits(seg).ceiling[1]));
    } else if (d.part === 'move') {
      const k = clamp(Math.pow(display.max_hz / display.min_hz, (x - d.sx) / ag.w), HZ_MIN / o.freq_lo, HZ_MAX / o.freq_hi);
      seg.freq_lo = roundHz(o.freq_lo * k);
      seg.freq_hi = roundHz(o.freq_hi * k);
      const limits = dbLimits(seg);
      const shift = roundDb(clamp(yd(y) - yd(d.sy), limits.threshold[0] - o.threshold, limits.ceiling[1] - o.ceiling));
      seg.threshold = o.threshold + shift;
      seg.ceiling = o.ceiling + shift;
    }
    changed(seg);
  });

  const finish = () => {
    const d = dragging;
    if (!d || d.area !== 'an') return;
    dragging = null;
    anCanvas.style.cursor = 'crosshair';
    if (d.part === 'create') {
      if (Math.abs(d.x - d.sx) < 8) return;
      const f1 = xf(Math.min(d.sx, d.x));
      const f2 = xf(Math.max(d.sx, d.x));
      let threshold = yd(Math.max(d.sy, d.y));
      let ceiling = yd(Math.min(d.sy, d.y));
      if (ceiling - threshold < 6) {
        threshold = -60;
        ceiling = -25;
      }
      const range = freeLedRange();
      addSegment({
        freq_lo: roundHz(f1), freq_hi: roundHz(f2),
        threshold: roundDb(threshold), ceiling: roundDb(ceiling),
        start: range[0], end: range[1],
      });
      return;
    }
    commit();
  };
  anCanvas.addEventListener('pointerup', finish);
  anCanvas.addEventListener('pointercancel', finish);
  anCanvas.addEventListener('pointerleave', () => { if (!dragging) $('#an-readout').textContent = ''; });
}

/* ---------- LED strip and segment track ---------- */

const stripCanvas = $('#strip');
const glowCanvas = $('#strip-glow');
const trackCanvas = $('#track');
const ST_PAD = 10;
const LANE_H = 28;
const LANE_GAP = 6;

function ledGeom(w) {
  const n = config.led_count;
  const cw = (w - ST_PAD * 2) / n;
  return {
    n,
    cw,
    x: (i) => ST_PAD + i * cw,
    idx: (x) => clamp(Math.floor((x - ST_PAD) / cw), 0, n - 1),
    edge: (x) => clamp(Math.round((x - ST_PAD) / cw), 0, n),
  };
}

// The LED colors as one row of pixels (unlit LEDs transparent), stretched over
// the strip and its glow with smoothing off: two drawImage calls per frame
// instead of a rounded rectangle per LED
const ledRow = { canvas: document.createElement('canvas'), image: null };
const ledShapes = {};

function ledRowImage(n) {
  const row = ledRow;
  if (!row.image || row.image.width !== n) {
    row.canvas.width = n;
    row.canvas.height = 1;
    row.image = row.canvas.getContext('2d').createImageData(n, 1);
  }
  const px = row.image.data;
  for (let i = 0; i < n; i++) {
    const r = ledBytes[i * 3] || 0;
    const g = ledBytes[i * 3 + 1] || 0;
    const b = ledBytes[i * 3 + 2] || 0;
    px[i * 4] = r;
    px[i * 4 + 1] = g;
    px[i * 4 + 2] = b;
    px[i * 4 + 3] = r + g + b > 0 ? 255 : 0;
  }
  row.canvas.getContext('2d').putImageData(row.image, 0, 0);
  return row.canvas;
}

function drawStrip() {
  const { ctx, w, h: hgt } = fitCanvas(stripCanvas);
  const glow = fitCanvas(glowCanvas);
  const g = ledGeom(w);
  const leds = ledRowImage(g.n);
  ctx.clearRect(0, 0, w, hgt);
  // Unlit LED shapes, then the colors painted only onto them
  ctx.drawImage(cachedLayer(ledShapes, stripCanvas, g.n, (c) => {
    const gap = g.cw > 6 ? 1.5 : g.cw > 3 ? 0.75 : 0;
    c.fillStyle = '#17171f';
    for (let i = 0; i < g.n; i++) {
      rrect(c, g.x(i) + gap / 2, 8, Math.max(0.5, g.cw - gap), hgt - 16, Math.min(3, g.cw / 3));
      c.fill();
    }
  }), 0, 0, w, hgt);
  ctx.imageSmoothingEnabled = false;
  ctx.globalCompositeOperation = 'source-atop';
  ctx.drawImage(leds, ST_PAD, 0, g.n * g.cw, hgt);
  ctx.globalCompositeOperation = 'source-over';

  glow.ctx.clearRect(0, 0, glow.w, glow.h);
  glow.ctx.imageSmoothingEnabled = false;
  glow.ctx.drawImage(leds, ST_PAD, 14, g.n * g.cw, glow.h - 28);

  const seg = selected();
  if (seg) {
    const x0 = g.x(seg.start);
    const x1 = g.x(Math.min(seg.end, g.n - 1) + 1);
    ctx.strokeStyle = 'rgba(255,255,255,0.75)';
    ctx.lineWidth = 1.5;
    rrect(ctx, x0 + 0.75, 3.75, x1 - x0 - 1.5, hgt - 7.5, 5);
    ctx.stroke();
  }
}

// Rows of the LED track. A segment keeps the row it was dragged to (seg.lane),
// the others take the first row where they fit. A segment that would overlap
// another in its row moves down (`last` is the one to move, e.g. the one just
// dragged), and empty rows close up.
function computeLanes(last = null) {
  const sorted = [...config.segments].sort((a, b) => a.start - b.start || a.end - b.end);
  const rows = [];
  const lanes = {};
  const place = (s, from) => {
    let row = from;
    while ((rows[row] || []).some((o) => s.start <= o.end && s.end >= o.start)) row++;
    (rows[row] = rows[row] || []).push(s);
    lanes[s.id] = row;
  };
  const pinned = (s) => s.lane >= 0;
  for (const s of sorted) if (pinned(s) && s.id !== last) place(s, s.lane);
  for (const s of sorted) if (pinned(s) && s.id === last) place(s, s.lane);
  for (const s of sorted) if (!pinned(s)) place(s, 0);
  const used = [...rows.keys()].filter((row) => rows[row] && rows[row].length);
  const compact = new Map(used.map((row, i) => [row, i]));
  for (const id of Object.keys(lanes)) lanes[id] = compact.get(lanes[id]);
  return { lanes, count: used.length };
}

// Pins every segment to the row it shows on
function settleLanes(last = null) {
  const { lanes } = computeLanes(last);
  for (const s of config.segments) s.lane = lanes[s.id];
}

// After a drag: the others keep the rows they had when it started, the
// dragged segment goes to `row` (or the first free row below it)
function pinLanes(base, id = null, row = 0) {
  for (const s of config.segments) if (s.id in base.lanes) s.lane = base.lanes[s.id];
  const seg = id && segById(id);
  if (seg) seg.lane = row;
  settleLanes(id);
}

// Row under a y position of the track, up to the empty row below the others
const laneAt = (y, count) => clamp(Math.floor((y - 4 + LANE_GAP / 2) / (LANE_H + LANE_GAP)), 0, count);

function currentLanes() {
  return dragging && dragging.area === 'track' && dragging.lanes ? dragging.lanes : computeLanes();
}

const laneY = (lane) => 4 + lane * (LANE_H + LANE_GAP);

function drawTrack() {
  const lanes = currentLanes();
  const wanted = laneY(lanes.count + 1) + 2;
  if (Math.abs(trackCanvas.clientHeight - wanted) > 0.5) trackCanvas.style.height = `${wanted}px`;
  const { ctx, w, h: hgt } = fitCanvas(trackCanvas);
  const g = ledGeom(w);
  ctx.clearRect(0, 0, w, hgt);

  // LED ticks
  ctx.font = '10px ui-monospace, Consolas, monospace';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'bottom';
  const step = g.cw * 10 >= 36 ? 10 : g.cw * 25 >= 36 ? 25 : 50;
  for (let i = 0; i <= g.n; i += step) {
    const x = Math.round(g.x(i)) + 0.5;
    ctx.strokeStyle = 'rgba(255,255,255,0.05)';
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, hgt);
    ctx.stroke();
  }

  // Empty lane for creating segments
  const emptyY = laneY(lanes.count);
  ctx.setLineDash([4, 4]);
  ctx.strokeStyle = 'rgba(255,255,255,0.1)';
  rrect(ctx, ST_PAD + 0.5, emptyY + 0.5, w - ST_PAD * 2 - 1, LANE_H - 1, 7);
  ctx.stroke();
  ctx.setLineDash([]);
  if (!dragging) {
    ctx.fillStyle = '#5f5d78';
    ctx.textBaseline = 'middle';
    ctx.font = '12px system-ui, sans-serif';
    ctx.fillText('Drag here to add a segment, or drop one here for a new row', w / 2, emptyY + LANE_H / 2);
  }

  const solo = soloActive();
  for (const seg of config.segments) {
    const lane = lanes.lanes[seg.id] ?? lanes.count;
    const y = laneY(lane);
    const x0 = g.x(seg.start);
    const x1 = g.x(Math.min(seg.end, g.n - 1) + 1);
    const width = Math.max(2, x1 - x0);
    const isSelected = seg.id === selectedId;
    const dim = !seg.enabled || (solo && !seg.solo);
    const level = live.levels && live.levels[seg.id];

    ctx.globalAlpha = dim ? 0.35 : 1;
    const grad = ctx.createLinearGradient(x0, 0, x0 + width, 0);
    seg.palette.forEach((c, i) => grad.addColorStop(seg.palette.length > 1 ? i / (seg.palette.length - 1) : 0, c));
    ctx.fillStyle = grad;
    rrect(ctx, x0 + 1, y, width - 2, LANE_H, 7);
    ctx.fill();
    ctx.fillStyle = 'rgba(8,8,13,0.35)';
    ctx.fill();

    if (level) {
      ctx.fillStyle = 'rgba(255,255,255,0.85)';
      ctx.fillRect(x0 + 6, y + LANE_H - 5, (width - 12) * level[1], 2.5);
    }

    ctx.lineWidth = isSelected ? 2 : 1;
    ctx.strokeStyle = isSelected ? '#ffffff' : 'rgba(255,255,255,0.16)';
    rrect(ctx, x0 + 1, y, width - 2, LANE_H, 7);
    ctx.stroke();

    if (width > 36) {
      ctx.save();
      ctx.beginPath();
      ctx.rect(x0 + 6, y, width - 12, LANE_H);
      ctx.clip();
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';
      ctx.shadowColor = 'rgba(0,0,0,0.9)';
      ctx.shadowBlur = 3;
      ctx.fillStyle = '#fff';
      ctx.font = '600 12px system-ui, sans-serif';
      ctx.fillText(seg.name, x0 + 8, y + LANE_H / 2 - 1);
      const nameW = ctx.measureText(seg.name).width;
      ctx.font = '11px ui-monospace, Consolas, monospace';
      ctx.fillStyle = 'rgba(255,255,255,0.75)';
      ctx.fillText(`${seg.start}–${seg.end}`, x0 + 16 + nameW, y + LANE_H / 2 - 1);
      ctx.restore();
    }

    if (isSelected) {
      ctx.fillStyle = '#fff';
      for (const x of [x0 + 4, x1 - 4]) {
        rrect(ctx, x - 1.5, y + 8, 3, LANE_H - 16, 1.5);
        ctx.fill();
      }
    }
    ctx.globalAlpha = 1;
  }

  if (dragging && dragging.area === 'track' && dragging.part === 'create') {
    const a = Math.min(dragging.a, dragging.b);
    const b = Math.max(dragging.a, dragging.b);
    const x0 = g.x(a);
    const x1 = g.x(b + 1);
    ctx.setLineDash([5, 4]);
    ctx.strokeStyle = 'rgba(255,255,255,0.8)';
    ctx.fillStyle = 'rgba(255,255,255,0.08)';
    rrect(ctx, x0 + 1, laneY(dragging.row), x1 - x0 - 2, LANE_H, 7);
    ctx.fill();
    ctx.stroke();
    ctx.setLineDash([]);
  }
}

function hitTrack(x, y) {
  const lanes = currentLanes();
  const g = ledGeom(trackCanvas.clientWidth);
  const others = config.segments.filter((s) => s.id !== selectedId);
  const order = selected() ? [selected(), ...others] : others;
  for (const seg of order) {
    const ly = laneY(lanes.lanes[seg.id] ?? 0);
    if (y < ly || y > ly + LANE_H) continue;
    const x0 = g.x(seg.start);
    const x1 = g.x(seg.end + 1);
    if (x < x0 - 5 || x > x1 + 5) continue;
    const mid = (x0 + x1) / 2;
    if (x <= mid && x - x0 <= 7) return { seg, part: 'start' };
    if (x > mid && x1 - x <= 7) return { seg, part: 'end' };
    return { seg, part: 'move' };
  }
  return null;
}

function setupStrip() {
  trackCanvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    const { x, y } = localPos(e, trackCanvas);
    const g = ledGeom(trackCanvas.clientWidth);
    const hit = hitTrack(x, y);
    trackCanvas.setPointerCapture(e.pointerId);
    const lanes = computeLanes();
    if (hit) {
      select(hit.seg.id);
      dragging = { area: 'track', part: hit.part, id: hit.seg.id, sx: x, orig: { ...hit.seg }, lanes, base: lanes, row: lanes.lanes[hit.seg.id] };
    } else {
      // A new segment goes on the row it's drawn on
      const i = g.idx(x);
      dragging = { area: 'track', part: 'create', sx: x, a: i, b: i, lanes, row: laneAt(y, lanes.count) };
    }
  });

  trackCanvas.addEventListener('pointermove', (e) => {
    const { x, y } = localPos(e, trackCanvas);
    const g = ledGeom(trackCanvas.clientWidth);
    const readout = (text) => setReadout($('#strip-readout'), text, x, trackCanvas.clientWidth);
    if (!dragging || dragging.area !== 'track') {
      const hit = hitTrack(x, y);
      trackCanvas.style.cursor = hit ? (hit.part === 'move' ? 'grab' : 'ew-resize') : 'crosshair';
      readout(`LED ${g.idx(x)}${hit ? ` · ${hit.seg.name}` : ''}`);
      return;
    }
    const d = dragging;
    if (d.part === 'create') {
      d.b = g.idx(x);
      readout(`LED ${Math.min(d.a, d.b)}–${Math.max(d.a, d.b)}`);
      return;
    }
    const seg = segById(d.id);
    if (!seg) return;
    const o = d.orig;
    if (d.part === 'start') {
      seg.start = clamp(g.edge(x), 0, seg.end);
    } else if (d.part === 'end') {
      seg.end = clamp(g.edge(x) - 1, seg.start, g.n - 1);
    } else {
      trackCanvas.style.cursor = 'grabbing';
      const len = o.end - o.start;
      seg.start = clamp(o.start + Math.round((x - d.sx) / g.cw), 0, g.n - 1 - len);
      seg.end = seg.start + len;
      // Up or down moves it to another row, or a new one below the others
      d.row = laneAt(y, d.base.count);
      d.lanes = { lanes: { ...d.base.lanes, [seg.id]: d.row }, count: Math.max(d.base.count, d.row + 1) };
    }
    readout(`${seg.name} · LED ${seg.start}–${seg.end} (${seg.end - seg.start + 1}) · row ${d.row + 1}`);
    changed(seg);
  });

  const finish = () => {
    const d = dragging;
    if (!d || d.area !== 'track') return;
    dragging = null;
    trackCanvas.style.cursor = 'crosshair';
    if (d.part === 'create') {
      if (Math.abs(d.b - d.a) < 1) {
        select(null);
        return;
      }
      pinLanes(d.lanes);
      addSegment({ start: Math.min(d.a, d.b), end: Math.max(d.a, d.b), lane: d.row });
      settleLanes(selectedId);
      commit();
      return;
    }
    pinLanes(d.base, d.id, d.row);
    commit();
  };
  trackCanvas.addEventListener('pointerup', finish);
  trackCanvas.addEventListener('pointercancel', finish);
  trackCanvas.addEventListener('pointerleave', () => { if (!dragging) $('#strip-readout').textContent = ''; });

  // Clicking a LED selects the segment on top of it
  stripCanvas.addEventListener('click', (e) => {
    const g = ledGeom(stripCanvas.clientWidth);
    const i = g.idx(localPos(e, stripCanvas).x);
    const covering = config.segments.filter((s) => s.start <= i && s.end >= i);
    if (covering.length) {
      const current = covering.findIndex((s) => s.id === selectedId);
      select(covering[(current + 1) % covering.length].id);
    }
  });
  stripCanvas.addEventListener('pointermove', (e) => {
    const g = ledGeom(stripCanvas.clientWidth);
    const { x } = localPos(e, stripCanvas);
    const i = g.idx(x);
    const names = config.segments.filter((s) => s.start <= i && s.end >= i).map((s) => s.name);
    setReadout($('#strip-readout'), `LED ${i}${names.length ? ` · ${names.join(', ')}` : ''}`, x, stripCanvas.clientWidth);
    stripCanvas.style.cursor = names.length ? 'pointer' : 'default';
  });
  stripCanvas.addEventListener('pointerleave', () => { $('#strip-readout').textContent = ''; });

  for (const button of document.querySelectorAll('.add-kind')) {
    button.addEventListener('click', () => addKind(button.dataset.kind));
  }
  $('#track-delete').addEventListener('click', deleteSelected);
}

// The track's Delete button names the segment it removes
function syncTrackBar() {
  const seg = selected();
  const button = $('#track-delete');
  const label = seg ? `Delete ${seg.name}` : 'Delete';
  if (button.textContent !== label) button.textContent = label;
  button.disabled = !seg;
}

/* ---------- segment list ---------- */

function renderList() {
  const list = $('#seg-list');
  list.replaceChildren();
  listRows.clear();
  if (!config.segments.length) {
    list.append(h('p', { class: 'empty-list' }, 'No segments yet. Use + Add or Auto split, or drag on the LED track or the spectrum.'));
    return;
  }
  for (const seg of config.segments) {
    const id = seg.id;
    const swatch = h('span', { class: 'swatch' });
    const name = h('strong');
    const meta = h('small');
    const meterBar = h('i');
    const solo = h('button', { type: 'button', class: 'solo', title: 'Solo: only soloed segments light up' }, 'S');
    const enabled = h('input', { type: 'checkbox', role: 'switch' });
    const row = h('div', { class: 'seg-row', tabindex: '0', role: 'button' }, [
      swatch,
      h('div', { class: 'meta' }, [name, meta]),
      h('div', { class: 'mini-meter' }, meterBar),
      solo,
      h('label', { class: 'switch', title: 'On / off' }, [enabled, h('span', { class: 'track' })]),
    ]);
    row.addEventListener('click', (e) => {
      if (!e.target.closest('button, label, input')) select(id);
    });
    row.addEventListener('keydown', (e) => {
      if (e.target === row && (e.key === 'Enter' || e.key === ' ')) {
        e.preventDefault();
        select(id);
      }
    });
    solo.addEventListener('click', () => {
      const s = segById(id);
      s.solo = !s.solo;
      changed(s);
    });
    enabled.addEventListener('change', () => {
      const s = segById(id);
      s.enabled = enabled.checked;
      changed(s);
    });
    listRows.set(id, { row, swatch, name, meta, meterBar, solo, enabled });
    list.append(row);
    updateListRow(seg);
  }
}

function updateListRow(seg) {
  const r = listRows.get(seg.id);
  if (!r) return;
  const accent = accentOf(seg.palette);
  const mode = MODES.find(([m]) => m === seg.mode);
  r.row.style.setProperty('--seg-row', `rgb(${accent.join(',')})`);
  r.swatch.style.background = gradientCss(seg.palette);
  r.name.textContent = seg.name;
  r.meta.textContent = `LED ${seg.start}–${seg.end} · ${fmtHz(seg.freq_lo)}–${fmtHz(seg.freq_hi)} · ${mode ? mode[1] : seg.mode}${seg.trigger === 'hits' ? ' · Hits' : ''}`;
  r.solo.setAttribute('aria-pressed', String(seg.solo));
  r.solo.setAttribute('aria-label', `Solo ${seg.name}`);
  r.enabled.checked = seg.enabled;
  r.enabled.setAttribute('aria-label', `${seg.name} on`);
  r.row.setAttribute('aria-label', `Select ${seg.name}`);
  r.row.classList.toggle('selected', seg.id === selectedId);
  r.row.classList.toggle('muted', !seg.enabled);
}

function select(id) {
  if (id === selectedId) return;
  selectedId = id;
  for (const seg of config.segments) updateListRow(seg);
  renderInspector();
}

/* ---------- inspector ---------- */

function syncInspector() {
  syncReactive();
  syncPresets();
  syncColorRanges();
  const seg = selected();
  if (!seg) return;
  const accent = accentOf(seg.palette);
  $('#inspector').style.setProperty('--seg', `rgb(${accent.join(',')})`);
  for (const sync of syncers) sync(seg);
}

function section(title, children) {
  return h('section', { class: 'insp-sec' }, [h('h3', null, title), ...children]);
}

// log: logarithmic slider (needs min > 0); pow: slider position^pow, for
// ranges whose small values need the most room
function rangeField({ key, label, min, max, step, unit = '', scale = 1, digits = 0, log = false, pow = 0 }) {
  const id = `f-${key}`;
  const curved = log || pow;
  const range = h('input', {
    type: 'range',
    min: curved ? 0 : min * scale,
    max: curved ? 1000 : max * scale,
    step: curved ? 1 : step * scale,
    'aria-label': label,
  });
  const num = h('input', { type: 'number', id, min: min * scale, max: max * scale, step: step * scale });
  const toSlider = (v) => {
    if (log) return (Math.log(v / min) / Math.log(max / min)) * 1000;
    if (pow) return Math.pow(clamp((v - min) / (max - min), 0, 1), 1 / pow) * 1000;
    return v * scale;
  };
  const fromSlider = (s) => {
    if (log) return roundHz(min * Math.pow(max / min, s / 1000));
    if (pow) return Math.round((min + (max - min) * Math.pow(s / 1000, pow)) / step) * step;
    return s / scale;
  };
  range.addEventListener('input', () => setValue(key, fromSlider(Number(range.value))));
  num.addEventListener('change', () => {
    const v = Number(num.value);
    if (num.value !== '' && Number.isFinite(v)) setValue(key, clamp(v / scale, min, max));
    else syncInspector();
  });
  syncers.push((seg) => {
    if (document.activeElement !== range) range.value = toSlider(seg[key]);
    if (document.activeElement !== num) num.value = Number((seg[key] * scale).toFixed(digits));
  });
  return h('div', { class: 'field' }, [
    h('label', { for: id }, label),
    h('div', { class: 'ctl' }, [range, num, h('span', { class: 'unit slot' }, unit)]),
  ]);
}

function segmented(key, options, label, variant = '') {
  const buttons = options.map(([value, text, title]) => {
    const button = h('button', { type: 'button', title: title || text, 'aria-label': title || text }, text);
    button.addEventListener('click', () => setValue(key, value));
    return [value, button];
  });
  syncers.push((seg) => {
    for (const [value, button] of buttons) button.setAttribute('aria-pressed', String(seg[key] === value));
  });
  return h('div', { class: `seg-ctl ${variant}`.trim(), role: 'group', 'aria-label': label }, buttons.map(([, b]) => b));
}

function toggle(key, label) {
  const input = h('input', { type: 'checkbox', role: 'switch' });
  input.addEventListener('change', () => setValue(key, input.checked));
  syncers.push((seg) => { input.checked = !!seg[key]; });
  return h('label', { class: 'switch' }, [input, h('span', { class: 'track' }), h('span', { class: 'switch-label' }, label)]);
}

function paletteEditor() {
  const chips = h('div', { class: 'chips' });
  const render = () => {
    const seg = selected();
    if (!seg) return;
    const items = seg.palette.map((color, i) => {
      const chip = h('div', { class: 'chip', style: `background:${color}` });
      const input = h('input', { type: 'color', value: color.toLowerCase(), 'aria-label': `Color ${i + 1} of ${seg.palette.length}` });
      input.addEventListener('input', () => {
        const s = selected();
        s.palette[i] = input.value.toUpperCase();
        chip.style.background = input.value;
        changed(s);
      });
      chip.append(input);
      if (seg.palette.length > 1) {
        const remove = h('button', { type: 'button', class: 'chip-x', 'aria-label': `Remove color ${i + 1}`, title: 'Remove' }, '×');
        remove.addEventListener('click', () => {
          const s = selected();
          s.palette.splice(i, 1);
          render();
          changed(s);
        });
        chip.append(remove);
      }
      return chip;
    });
    const add = h('button', { type: 'button', class: 'chip-add', 'aria-label': 'Add color', title: 'Add color' }, '+');
    add.disabled = seg.palette.length >= 16;
    add.addEventListener('click', () => {
      const s = selected();
      s.palette.push(s.palette[s.palette.length - 1]);
      render();
      changed(s);
    });
    chips.replaceChildren(...items, add);
  };
  render();

  const presets = h('div', { class: 'presets' }, Object.entries(PRESETS).map(([name, colors]) => {
    const button = h('button', { type: 'button', class: 'preset' }, [h('span', { style: `background:${gradientCss(colors)}` }), name]);
    button.addEventListener('click', () => {
      const s = selected();
      s.palette = [...colors];
      render();
      changed(s);
    });
    return button;
  }));
  const reverse = h('button', { type: 'button', class: 'preset' }, '⇄ Reverse');
  reverse.addEventListener('click', () => {
    const s = selected();
    s.palette.reverse();
    render();
    changed(s);
  });
  presets.append(reverse);
  return [chips, presets];
}

function renderInspector() {
  const root = $('#inspector');
  syncers = [];
  meterRefs = null;
  root.replaceChildren();
  const seg = selected();
  if (!seg) {
    root.append(h('div', { class: 'empty' }, [
      h('h2', null, 'No segment selected'),
      h('p', null, 'Pick one from the list, or drag on the LED track or the spectrum to create a segment.'),
    ]));
    syncReactive();
    syncPresets();
    syncColorRanges();
    return;
  }

  // Header
  const swatch = h('span', { class: 'swatch' });
  const name = h('input', { type: 'text', class: 'name-input', maxlength: '40', 'aria-label': 'Segment name' });
  name.addEventListener('input', () => {
    const s = selected();
    s.name = name.value.trim() || 'Segment';
    updateListRow(s);
    commit();
  });
  const solo = h('button', { type: 'button', class: 'solo', title: 'Solo: only soloed segments light up' }, 'S');
  solo.addEventListener('click', () => setValue('solo', !selected().solo));
  syncers.push((s) => {
    swatch.style.background = gradientCss(s.palette);
    if (document.activeElement !== name) name.value = s.name;
    solo.setAttribute('aria-pressed', String(s.solo));
    solo.setAttribute('aria-label', `Solo ${s.name}`);
  });

  // Frequency
  const bandChips = BANDS.map(([label, lo, hi]) => {
    const button = h('button', { type: 'button', class: 'chip-btn', title: `${fmtHz(lo)} – ${fmtHz(hi)}` }, label);
    button.addEventListener('click', () => {
      const s = selected();
      s.freq_lo = lo;
      s.freq_hi = hi;
      enforce(s, null);
      changed(s);
    });
    return [lo, hi, button];
  });
  syncers.push((s) => {
    for (const [lo, hi, button] of bandChips) {
      button.setAttribute('aria-pressed', String(Math.abs(s.freq_lo - lo) < 0.5 && Math.abs(s.freq_hi - hi) < 0.5));
    }
  });

  // Live meter
  const zone = h('i', { class: 'zone' });
  const raw = h('i', { class: 'raw' });
  const thrMark = h('i', { class: 'mark' });
  const ceilMark = h('i', { class: 'mark ceil' });
  const outBar = h('i');
  const levelText = h('span');
  const valueText = h('span');
  meterRefs = { zone, raw, thrMark, ceilMark, outBar, levelText, valueText, lastText: '' };

  // LEDs
  const ledCount = h('p', { class: 'note' });
  const modeHelp = h('p', { class: 'note' });
  const peakRow = h('div', { class: 'field' }, [h('span'), toggle('peak', 'Peak hold marker')]);
  const speedRow = rangeField({ key: 'speed', label: 'Speed', min: 0, max: 2000, step: 1, unit: 'LED/s', pow: 2.5 });
  const sizeRow = rangeField({ key: 'width', label: 'Size', min: 1, max: 100, step: 1, unit: '%' });
  const sizeLabel = sizeRow.querySelector('label');
  const sizeSlider = sizeRow.querySelector('input[type=range]');
  syncers.push((s) => {
    ledCount.textContent = `${s.end - s.start + 1} of ${config.led_count} LEDs`;
    modeHelp.textContent = MODE_HELP[s.mode] || '';
    peakRow.hidden = s.mode !== 'fill';
    speedRow.hidden = !SPEED_MODES.has(s.mode);
    sizeRow.hidden = !SIZE_LABEL[s.mode];
    sizeLabel.textContent = SIZE_LABEL[s.mode] || 'Size';
    sizeSlider.setAttribute('aria-label', sizeLabel.textContent);
  });

  // Threshold fields depend on the trigger; switching it rebuilds the panel
  const hits = seg.trigger === 'hits';
  const triggerHelp = h('p', { class: 'note' }, TRIGGER_HELP[seg.trigger]);
  const thresholdFields = hits
    ? [
      rangeField({ key: 'threshold', label: 'Above avg', min: 0, max: HITS_MAX_DB - 1, step: 0.5, unit: 'dB', digits: 1 }),
      rangeField({ key: 'ceiling', label: 'Full at', min: 1, max: HITS_MAX_DB, step: 0.5, unit: 'dB', digits: 1 }),
    ]
    : [
      rangeField({ key: 'threshold', label: 'Threshold', min: -120, max: -1, step: 0.5, unit: 'dB', digits: 1 }),
      rangeField({ key: 'ceiling', label: 'Full at', min: -119, max: 0, step: 0.5, unit: 'dB', digits: 1 }),
    ];

  const n = config.led_count;
  const [chips, presets] = paletteEditor();

  const duplicate = h('button', { type: 'button', class: 'btn' }, 'Duplicate');
  duplicate.addEventListener('click', duplicateSelected);
  const remove = h('button', { type: 'button', class: 'btn ghost danger' }, 'Delete');
  confirmClick(remove, 'Confirm delete', deleteSelected);

  root.append(
    h('div', { class: 'insp-head' }, [swatch, name, solo, toggle('enabled', 'On')]),
    section('Reacts to', [
      h('div', { class: 'band-chips', role: 'group', 'aria-label': 'Frequency band presets' }, bandChips.map(([, , b]) => b)),
      rangeField({ key: 'freq_lo', label: 'Low', min: HZ_MIN, max: HZ_MAX, step: 1, unit: 'Hz', log: true, digits: 1 }),
      rangeField({ key: 'freq_hi', label: 'High', min: HZ_MIN, max: HZ_MAX, step: 1, unit: 'Hz', log: true, digits: 1 }),
      h('div', { class: 'stack' }, [h('span', { class: 'stack-label' }, 'Trigger'), segmented('trigger', TRIGGERS, 'Trigger'), triggerHelp]),
      ...thresholdFields,
      h('div', { class: 'meter', role: 'img', 'aria-label': 'Band level against threshold and full brightness' }, [zone, raw, thrMark, ceilMark]),
      h('div', { class: 'out-meter', role: 'img', 'aria-label': 'Segment output' }, outBar),
      h('div', { class: 'meter-text mono' }, [levelText, valueText]),
    ]),
    section('Response', [
      rangeField({ key: 'gain', label: 'Gain', min: 0.1, max: 8, step: 0.05, unit: '×', digits: 2 }),
      rangeField({ key: 'curve', label: 'Curve', min: 0.2, max: 5, step: 0.05, digits: 2 }),
      rangeField({ key: 'attack', label: 'Attack', min: 0, max: 1000, step: 1, unit: 'ms' }),
      rangeField({ key: 'release', label: 'Release', min: 0, max: 3000, step: 10, unit: 'ms' }),
      rangeField({ key: 'floor', label: 'Idle glow', min: 0, max: 1, step: 0.01, scale: 100, unit: '%' }),
    ]),
    section('LEDs', [
      rangeField({ key: 'start', label: 'First LED', min: 0, max: n - 1, step: 1 }),
      rangeField({ key: 'end', label: 'Last LED', min: 0, max: n - 1, step: 1 }),
      ledCount,
      h('div', { class: 'stack' }, [h('span', { class: 'stack-label' }, 'Mode'), segmented('mode', MODES, 'Mode', 'wrap'), modeHelp]),
      h('div', { class: 'stack' }, [h('span', { class: 'stack-label' }, 'Direction'), segmented('direction', DIRECTIONS, 'Direction')]),
      peakRow,
      speedRow,
      sizeRow,
    ]),
    section('Colors', [chips, presets]),
    h('div', { class: 'insp-foot' }, [duplicate, remove]),
  );
  syncInspector();
}

// Meter parts span their whole track and move by transform only, which skips
// layout; unchanged values are not written again
function setMeter(el, transform) {
  if (el.dataset.t !== transform) {
    el.dataset.t = transform;
    el.style.transform = transform;
  }
}

const meterScale = (fraction) => `scaleX(${clamp(fraction, 0, 1).toFixed(3)})`;

function updateMeters() {
  const seg = selected();
  if (seg && meterRefs) {
    const level = live.levels && live.levels[seg.id];
    const pct = (d) => clamp((d - DB_MIN) / (DB_MAX - DB_MIN), 0, 1) * 100;
    const m = meterRefs;
    const base = dbBase(seg);
    const lo = pct(base + seg.threshold);
    const hi = pct(base + seg.ceiling);
    setMeter(m.zone, `translateX(${lo.toFixed(2)}%) ${meterScale((hi - lo) / 100)}`);
    setMeter(m.thrMark, `translateX(${lo.toFixed(2)}%)`);
    setMeter(m.ceilMark, `translateX(${hi.toFixed(2)}%)`);
    setMeter(m.raw, meterScale(level ? pct(level[0]) / 100 : 0));
    setMeter(m.outBar, meterScale(level ? level[1] : 0));
    const text = level ? `${level[0].toFixed(1)} dB|${Math.round(level[1] * 100)}% out` : '–|–';
    if (text !== m.lastText) {
      m.lastText = text;
      [m.levelText.textContent, m.valueText.textContent] = text.split('|');
    }
  }
  for (const [id, row] of listRows) {
    const level = live.levels && live.levels[id];
    setMeter(row.meterBar, meterScale(level ? level[1] : 0));
  }
}

/* ---------- reactive presets ---------- */

const TEMPO_CLASS = { 'Very fast': 'very-fast', Fast: 'fast', Medium: 'medium', Slow: 'slow' };

function renderReactive() {
  reactiveTiles = [];
  const groups = REACTIVE.map(([group, presets]) => {
    const tiles = presets.map((preset) => {
      const bars = Array.from({ length: 5 }, (_, i) => h('i', { style: `--i:${i}` }));
      const tile = h('button', { type: 'button', class: 'rx-tile', 'aria-pressed': 'false' }, [
        h('span', { class: `rx-preview anim-${preset.anim}`, style: `--dur:${preset.dur}s`, 'aria-hidden': 'true' }, bars),
        h('span', { class: 'rx-text' }, [
          h('strong', null, preset.name),
          h('small', null, `${fmtHz(preset.range.freq_lo)}–${fmtHz(preset.range.freq_hi)} · ${preset.desc}`),
        ]),
        h('span', { class: `rx-tempo ${TEMPO_CLASS[preset.tempo]}` }, preset.tempo),
      ]);
      tile.addEventListener('click', () => applyReactive(preset));
      reactiveTiles.push([preset, tile]);
      return tile;
    });
    return h('section', { class: 'rx-group' }, [h('h3', null, group), h('div', { class: 'rx-grid' }, tiles)]);
  });
  $('#rx-groups').replaceChildren(...groups);
  syncReactive();
}

const sameValue = (a, b) => (typeof a === 'number' ? Math.abs(a - b) < 0.01 : a === b);

// Highlights the presets the selected segment currently matches
function syncReactive() {
  const seg = selected();
  const keep = $('#rx-keep').checked;
  const card = $('#reactive');
  card.style.setProperty('--seg', seg ? `rgb(${accentOf(seg.palette).join(',')})` : 'var(--accent)');
  for (const [preset, tile] of reactiveTiles) {
    const match = !!seg && reactiveScope === 'selected'
      && RESPONSE_KEYS.every((k) => sameValue(seg[k], preset.response[k]))
      && (keep || RANGE_KEYS.every((k) => sameValue(seg[k], preset.range[k])));
    tile.setAttribute('aria-pressed', String(match));
  }

  const note = $('#rx-note');
  const keepWrap = $('#rx-keep-wrap');
  keepWrap.hidden = reactiveScope !== 'selected';
  if (reactiveScope === 'all') note.textContent = 'Applies the motion to every segment. Each keeps its own frequency range and threshold.';
  else if (reactiveScope === 'new') note.textContent = 'Adds a new segment with the preset on the longest free stretch of LEDs.';
  else if (!seg) note.textContent = 'No segment selected: a preset adds a new segment.';
  else note.textContent = keep ? `Applies the motion to ${seg.name}, keeping its frequency range and threshold.` : `Applies to ${seg.name}.`;
}

function applyReactive(preset) {
  const clone = (o) => JSON.parse(JSON.stringify(o));
  const seg = selected();
  if (reactiveScope === 'new' || (reactiveScope === 'selected' && !seg)) {
    addSegment({ name: uniqueName(preset.name), ...clone(preset.range), ...clone(preset.response) });
    toast(`Added ${selected().name}`);
    return;
  }
  if (reactiveScope === 'all') {
    if (!config.segments.length) {
      toast('There are no segments yet');
      return;
    }
    for (const s of config.segments) {
      Object.assign(s, clone(preset.response));
      updateListRow(s);
    }
    toast(`${preset.name} applied to all ${config.segments.length} segments`);
  } else {
    Object.assign(seg, clone(preset.response));
    if (!$('#rx-keep').checked) Object.assign(seg, clone(preset.range));
    enforce(seg, null);
    updateListRow(seg);
    toast(`${preset.name} applied to ${seg.name}`);
  }
  syncInspector();
  syncReactive();
  commit();
}

function setupReactive() {
  const scope = $('#rx-scope');
  scope.addEventListener('click', (e) => {
    const button = e.target.closest('button[data-scope]');
    if (!button) return;
    reactiveScope = button.dataset.scope;
    for (const b of scope.querySelectorAll('button')) b.setAttribute('aria-pressed', String(b === button));
    syncReactive();
  });
  $('#rx-keep').addEventListener('change', syncReactive);
  renderReactive();
}

/* ---------- effects (whole-strip looks) ---------- */

const LOWS_PAL = ['#0033CC', '#000080', '#4B00FF'];
const MIDS_PAL = ['#FF3399', '#FF00FF', '#800080'];
const HIGHS_PAL = ['#800080', '#8F00FF', '#4B0082'];

// How a segment reacts: frequency range, trigger, thresholds and motion.
// Hits thresholds are dB above the band's recent average; level thresholds
// are tuned for audio coming out of SteelSeries Sonar.
const REACT = {
  lowsFlicker: { freq_lo: 35, freq_hi: 150, trigger: 'hits', threshold: 2, ceiling: 10, mode: 'pulse', attack: 0, release: 45, curve: 1.6 },
  lowsStrobe: { freq_lo: 35, freq_hi: 120, trigger: 'hits', threshold: 4, ceiling: 12, mode: 'pulse', attack: 0, release: 35, curve: 2 },
  lowsBurst: { freq_lo: 35, freq_hi: 150, trigger: 'hits', threshold: 2, ceiling: 10, mode: 'fill', attack: 0, release: 140, curve: 1.2, peak: true },
  midsPulse: { freq_lo: 250, freq_hi: 2000, trigger: 'hits', threshold: 1, ceiling: 9, mode: 'pulse', attack: 25, release: 300, curve: 1.2 },
  highsFlicker: { freq_lo: 5000, freq_hi: 16000, trigger: 'hits', threshold: 4, ceiling: 13, mode: 'pulse', attack: 0, release: 40, curve: 1.3 },
  highsSparkle: { freq_lo: 5000, freq_hi: 16000, trigger: 'hits', threshold: 4, ceiling: 14, mode: 'spectrum', attack: 0, release: 110, curve: 1.3 },
  highsFill: { freq_lo: 4000, freq_hi: 16000, trigger: 'hits', threshold: 3, ceiling: 13, mode: 'fill', attack: 0, release: 160, curve: 1.2 },
  bandPulse: { trigger: 'hits', threshold: 2, ceiling: 11, mode: 'pulse', attack: 5, release: 180, curve: 1.2 },
  fullSpectrum: { freq_lo: 30, freq_hi: 14000, trigger: 'hits', threshold: 2, ceiling: 12, mode: 'spectrum', attack: 0, release: 150, curve: 1.2 },
  vu: { freq_lo: 30, freq_hi: 16000, trigger: 'level', threshold: -38, ceiling: -14, mode: 'fill', attack: 5, release: 300, peak: true },
  drift: { freq_lo: 40, freq_hi: 4000, trigger: 'level', threshold: -42, ceiling: -14, mode: 'shift', attack: 400, release: 1500, floor: 0.45 },
  breathe: { freq_lo: 60, freq_hi: 2000, trigger: 'level', threshold: -40, ceiling: -16, mode: 'pulse', attack: 600, release: 2500, floor: 0.15 },
};

// One segment over the fraction from..to of an n LED strip
function part(n, name, from, to, react, palette, extra = {}) {
  const start = Math.round(from * n);
  return {
    id: newId(), name, enabled: true, solo: false,
    start, end: Math.max(start, Math.round(to * n) - 1),
    trigger: 'level', threshold: -60, ceiling: -25,
    gain: 1, curve: 1, attack: 15, release: 200, floor: 0,
    mode: 'pulse', direction: 'forward', peak: false, ...MOTION_DEFAULTS,
    ...react, ...extra, palette: [...palette],
  };
}

const EFFECTS = [
  { name: 'Alternating flicker', tags: ['EDM', 'Pop', 'Strobe'], tempo: 'Fast', desc: '10 groups of highs, lows and mids in turn. Highs and lows flicker fast, mids pulse.',
    build: (n) => Array.from({ length: 10 }, (_, i) => {
      const num = Math.floor(i / 3) + 1;
      return [
        () => part(n, `Highs ${num}`, i / 10, (i + 1) / 10, REACT.highsFlicker, HIGHS_PAL),
        () => part(n, `Lows ${num}`, i / 10, (i + 1) / 10, REACT.lowsFlicker, LOWS_PAL, { direction: 'center' }),
        () => part(n, `Mids ${num}`, i / 10, (i + 1) / 10, REACT.midsPulse, MIDS_PAL, { direction: 'center' }),
      ][i % 3]();
    }) },
  { name: 'Strobe & sparkle', tags: ['Techno', 'EDM', 'Strobe'], tempo: 'Very fast', desc: 'The whole strip strobes blue on kicks with violet hi-hat sparkles on top.',
    build: (n) => [
      part(n, 'Kick strobe', 0, 1, REACT.lowsStrobe, LOWS_PAL, { direction: 'center' }),
      part(n, 'Sparkles', 0, 1, REACT.highsSparkle, HIGHS_PAL),
    ] },
  { name: 'Bass burst', tags: ['Dubstep', 'Trap', 'EDM'], tempo: 'Fast', desc: 'Bass bursts out from the middle, mids glow beside it, sparkles at both ends.',
    build: (n) => [
      part(n, 'Sparkle left', 0, 0.2, REACT.highsSparkle, HIGHS_PAL, { direction: 'reverse' }),
      part(n, 'Glow left', 0.2, 0.35, REACT.midsPulse, MIDS_PAL),
      part(n, 'Bass burst', 0.35, 0.65, REACT.lowsBurst, LOWS_PAL, { direction: 'center' }),
      part(n, 'Glow right', 0.65, 0.8, REACT.midsPulse, MIDS_PAL),
      part(n, 'Sparkle right', 0.8, 1, REACT.highsSparkle, HIGHS_PAL),
    ] },
  { name: 'Collide', tags: ['Drum & Bass', 'EDM'], tempo: 'Fast', desc: 'Bass rushes in from both ends while the highs flash where they meet.',
    build: (n) => [
      part(n, 'Bass from edges', 0, 1, REACT.lowsBurst, LOWS_PAL, { direction: 'edges' }),
      part(n, 'Center flash', 0.4, 0.6, REACT.highsFlicker, ['#FF3399', '#8F00FF', '#FF3399'], { direction: 'center' }),
    ] },
  { name: 'Mirror spectrum', tags: ['All genres', 'Spectrum'], tempo: 'Fast', desc: 'The whole spectrum from the middle out: bass in the center, treble at the ends.',
    build: (n) => [part(n, 'Spectrum', 0, 1, REACT.fullSpectrum, PRESETS['Deep Night'], { direction: 'center' })] },
  { name: 'VU meter', tags: ['Rock', 'Pop', 'Meter'], tempo: 'Fast', desc: 'One big volume meter growing from the middle, with a peak marker.',
    build: (n) => [part(n, 'VU meter', 0, 1, REACT.vu, PRESETS['Deep Night'], { direction: 'center' })] },
  { name: 'Equalizer bands', tags: ['Jazz', 'Classical', 'Spectrum'], tempo: 'Medium', desc: 'Seven bands from sub bass to air side by side, each pulsing on its own hits.',
    build: (n) => BANDS.map(([name, lo, hi], i) => {
      const night = PRESETS['Deep Night'];
      return part(n, name, i / BANDS.length, (i + 1) / BANDS.length,
        { ...REACT.bandPulse, freq_lo: lo, freq_hi: hi }, [night[i], night[i + 1]], { direction: 'center' });
    }) },
  { name: 'Bass vs treble', tags: ['Hip-hop', 'Rock'], tempo: 'Medium', desc: 'Bass fills up from one end, treble from the other, mids glow where they meet.',
    build: (n) => [
      part(n, 'Bass', 0, 0.5, REACT.lowsBurst, LOWS_PAL),
      part(n, 'Treble', 0.5, 1, REACT.highsFill, HIGHS_PAL, { direction: 'reverse' }),
      part(n, 'Mids', 0.45, 0.55, REACT.midsPulse, MIDS_PAL, { direction: 'center' }),
    ] },
  { name: 'Starfield', tags: ['Ambient', 'Lo-fi', 'Chill'], tempo: 'Medium', desc: 'Pink and violet sparkles twinkling over a slow navy glow.',
    build: (n) => [
      part(n, 'Night glow', 0, 1, { ...REACT.midsPulse, attack: 300, release: 900, floor: 0.12 }, ['#000080', '#4B0082', '#000080']),
      part(n, 'Stars', 0, 1, { ...REACT.highsSparkle, threshold: 5, ceiling: 15, release: 70 }, ['#FF3399', '#8F00FF', '#FF00FF']),
    ] },
  { name: 'Color drift', tags: ['Ambient', 'Classical', 'Chill'], tempo: 'Slow', desc: 'Calm colors drifting through your palette as the music swells.',
    build: (n) => [part(n, 'Color drift', 0, 1, REACT.drift, PRESETS['Deep Night'])] },
  { name: 'Breathing', tags: ['R&B', 'Lo-fi', 'Chill'], tempo: 'Slow', desc: 'The whole strip slowly breathes with the overall energy.',
    build: (n) => [part(n, 'Breathing', 0, 1, REACT.breathe, ['#4B00FF', '#FF00FF', '#4B00FF'], { direction: 'center' })] },
  { name: 'Ambient zones', tags: ['Ambient', 'Jazz', 'Chill'], tempo: 'Slow', desc: 'Three wide zones for lows, mids and highs, glowing softly for background listening.',
    build: (n) => [
      part(n, 'Lows zone', 0, 1 / 3, { ...REACT.breathe, freq_lo: 35, freq_hi: 200, threshold: -42, ceiling: -16, attack: 1200, release: 4000, floor: 0.3 }, LOWS_PAL, { direction: 'center' }),
      part(n, 'Mids zone', 1 / 3, 2 / 3, { ...REACT.breathe, freq_lo: 250, freq_hi: 2000, threshold: -48, ceiling: -26, attack: 1200, release: 4000, floor: 0.3 }, MIDS_PAL, { direction: 'center' }),
      part(n, 'Highs zone', 2 / 3, 1, { ...REACT.breathe, freq_lo: 4000, freq_hi: 16000, threshold: -60, ceiling: -36, attack: 1200, release: 4000, floor: 0.3 }, HIGHS_PAL, { direction: 'center' }),
    ] },
];

/* ---------- LedFx effects ---------- */

// Ports of LedFx's audio reactive effects (github.com/LedFx/LedFx,
// ledfx/effects), built from segments in Deep Night colors. LedFx splits the
// sound into lows (beat + bass), mids and highs; level thresholds are tuned
// for the hot Sonar output. LedFx's BPM effects use hits instead of its beat
// tracker.
const LFX_LOWS = { freq_lo: 30, freq_hi: 250, trigger: 'level', threshold: -48, ceiling: -14 };
const LFX_MIDS = { freq_lo: 250, freq_hi: 3000, trigger: 'level', threshold: -52, ceiling: -22 };
const LFX_HIGHS = { freq_lo: 3000, freq_hi: 10000, trigger: 'level', threshold: -64, ceiling: -34 };
const LFX_KICK = { freq_lo: 35, freq_hi: 150 };
const LFX_TOPS = { freq_lo: 2000, freq_hi: 10000 };
const hitsOn = (band, threshold, ceiling) => ({ ...band, trigger: 'hits', threshold, ceiling });
// Dark to bright, for modes that pick the color by level or heat
const LOWS_RAMP = ['#000080', '#0033CC', '#4B00FF'];
const MIDS_RAMP = ['#800080', '#FF00FF', '#FF3399'];
const HIGHS_RAMP = ['#4B0082', '#800080', '#8F00FF'];
const NIGHT_FIRE = ['#000080', '#4B00FF', '#8F00FF', '#FF00FF', '#FF3399'];

const logBands = (count, lo, hi) => Array.from({ length: count }, (_, i) => [
  roundHz(lo * Math.pow(hi / lo, i / count)), roundHz(lo * Math.pow(hi / lo, (i + 1) / count)),
]);

// One segment per band of the spectrum, side by side
function bandParts(n, count, react, palettes, extra = {}) {
  const night = PRESETS['Deep Night'];
  return logBands(count, 30, 15000).map(([lo, hi], i) => part(n, `Band ${i + 1}`, i / count, (i + 1) / count,
    { ...react(i), freq_lo: lo, freq_hi: hi }, palettes ? palettes[i] : [night[i % night.length], night[(i + 1) % night.length]], extra));
}

const LEDFX_EFFECTS = [
  { name: 'Energy', tempo: 'Fast', tags: ['EDM', 'Pop', 'Meter'],
    desc: 'Lows, mids and highs each fill out from the middle in their own color, layered on top of each other.',
    build: (n) => [
      part(n, 'Lows', 0, 1, { ...LFX_LOWS, mode: 'fill', attack: 0, release: 90 }, LOWS_PAL, { direction: 'center' }),
      part(n, 'Mids', 0, 1, { ...LFX_MIDS, mode: 'fill', attack: 0, release: 90 }, MIDS_PAL, { direction: 'center' }),
      part(n, 'Highs', 0, 1, { ...LFX_HIGHS, mode: 'fill', attack: 0, release: 90 }, HIGHS_PAL, { direction: 'center' }),
    ] },
  { name: 'Scroll', tempo: 'Fast', tags: ['EDM', 'House', 'Hip-hop'],
    desc: 'Lows, mids and highs stream out from the middle as colored trails that fade on the way.',
    build: (n) => [
      part(n, 'Lows', 0, 1, { ...LFX_LOWS, mode: 'scroll', attack: 0, release: 40, curve: 2, speed: 180, width: 100 }, LOWS_RAMP, { direction: 'center' }),
      part(n, 'Mids', 0, 1, { ...LFX_MIDS, mode: 'scroll', attack: 0, release: 40, curve: 2, speed: 180, width: 100 }, MIDS_RAMP, { direction: 'center' }),
      part(n, 'Highs', 0, 1, { ...LFX_HIGHS, mode: 'scroll', attack: 0, release: 40, curve: 2, speed: 180, width: 100 }, HIGHS_RAMP, { direction: 'center' }),
    ] },
  { name: 'Wavelength', tempo: 'Fast', tags: ['All genres', 'Spectrum'],
    desc: 'The whole spectrum laid along the strip in your palette, bass at the bottom, each LED following its own slice.',
    build: (n) => [part(n, 'Spectrum', 0, 1, { freq_lo: 30, freq_hi: 15000, trigger: 'level', threshold: -70, ceiling: -28, mode: 'spectrum', attack: 0, release: 200 }, PRESETS['Deep Night'])] },
  { name: 'Power', tempo: 'Fast', tags: ['EDM', 'Dubstep', 'Trap'],
    desc: 'A dim spectrum backdrop, the bass filling out from the middle over it, and sparks on the hi-hats.',
    build: (n) => [
      part(n, 'Backdrop', 0, 1, { freq_lo: 30, freq_hi: 15000, trigger: 'level', threshold: -72, ceiling: -24, mode: 'spectrum', attack: 0, release: 250, gain: 0.6 }, PRESETS['Deep Night'], { direction: 'center' }),
      part(n, 'Bass', 0, 1, { ...LFX_LOWS, mode: 'fill', attack: 0, release: 200 }, LOWS_PAL, { direction: 'center' }),
      part(n, 'Sparks', 0, 1, { ...hitsOn(LFX_TOPS, 3, 12), mode: 'spectrum', attack: 0, release: 90, curve: 1.5 }, ['#FF3399', '#FF00FF', '#FF3399']),
    ] },
  { name: 'Rain', tempo: 'Medium', tags: ['Pop', 'R&B', 'Lo-fi'],
    desc: 'Every hit in the lows, mids or highs drops a ripple at a random spot in its color.',
    build: (n) => [
      part(n, 'Lows', 0, 1, { ...hitsOn(LFX_LOWS, 3, 11), mode: 'drops', attack: 0, release: 500, speed: 40, width: 4 }, ['#0033CC', '#4B00FF']),
      part(n, 'Mids', 0, 1, { ...hitsOn(LFX_MIDS, 3, 11), mode: 'drops', attack: 0, release: 450, speed: 30, width: 3 }, ['#FF3399', '#FF00FF']),
      part(n, 'Highs', 0, 1, { ...hitsOn(LFX_HIGHS, 4, 13), mode: 'drops', attack: 0, release: 350, speed: 20, width: 2 }, ['#8F00FF', '#800080']),
    ] },
  { name: 'Spotlight', tempo: 'Medium', tags: ['House', 'Pop', 'Chill'],
    desc: 'Soft spots of light pop up at random and fade out, more of them when the music gets busy.',
    build: (n) => [
      part(n, 'Beat spots', 0, 1, { ...hitsOn(LFX_LOWS, 2, 10), mode: 'drops', attack: 0, release: 700, speed: 0, width: 8 }, PRESETS['Deep Night']),
      part(n, 'Accent spots', 0, 1, { ...hitsOn(LFX_TOPS, 3, 12), mode: 'drops', attack: 0, release: 500, speed: 0, width: 5 }, ['#FF3399', '#FF00FF', '#8F00FF']),
    ] },
  { name: 'Scan', tempo: 'Fast', tags: ['Techno', 'Rock'],
    desc: 'One eye bounces from end to end, racing with the bass and taking the color of where it is.',
    build: (n) => [part(n, 'Scanner', 0, 1, { ...LFX_LOWS, mode: 'scan', attack: 0, release: 250, floor: 0.15, speed: 500, width: 20 }, PRESETS['Deep Night'])] },
  { name: 'Scan multi', tempo: 'Fast', tags: ['Techno', 'EDM'],
    desc: 'Three eyes for the lows, mids and highs, each moving at the pace of its own range.',
    build: (n) => [
      part(n, 'Lows eye', 0, 1, { ...LFX_LOWS, mode: 'scan', attack: 0, release: 250, floor: 0.1, speed: 450, width: 15 }, LOWS_PAL),
      part(n, 'Mids eye', 0, 1, { ...LFX_MIDS, mode: 'scan', attack: 0, release: 250, floor: 0.1, speed: 400, width: 12 }, MIDS_PAL),
      part(n, 'Highs eye', 0, 1, { ...LFX_HIGHS, mode: 'scan', attack: 0, release: 250, floor: 0.1, speed: 350, width: 10 }, HIGHS_PAL),
    ] },
  { name: 'Strobe', tempo: 'Very fast', tags: ['Techno', 'EDM', 'Strobe'],
    desc: 'Kicks blast the whole strip in the next color from the middle out, with quick pink flashes on snares and hats.',
    build: (n) => [
      part(n, 'Bass strobe', 0, 1, { ...hitsOn(LFX_KICK, 3, 11), mode: 'wipe', attack: 0, release: 60, speed: 2000, width: 100 }, PRESETS['Deep Night'], { direction: 'center' }),
      part(n, 'Flashes', 0, 1, { ...hitsOn(LFX_TOPS, 4, 13), mode: 'drops', attack: 0, release: 60, speed: 0, width: 3.5 }, ['#FF3399', '#FF00FF']),
    ] },
  { name: 'Blade Power+', tempo: 'Fast', tags: ['Hip-hop', 'Trap', 'Meter'],
    desc: 'Your whole palette fills up from the bottom of the strip with the bass.',
    build: (n) => [part(n, 'Blade', 0, 1, { ...LFX_LOWS, mode: 'fill', attack: 0, release: 90 }, PRESETS['Deep Night'])] },
  { name: 'Bar', tempo: 'Fast', tags: ['House', 'Pop'],
    desc: 'A bar travels to the other end on every kick, turning around and changing color each time (hits stand in for LedFx’s beat tracking).',
    build: (n) => [part(n, 'Bar', 0, 1, { ...hitsOn(LFX_KICK, 2, 10), mode: 'wipe', attack: 0, release: 400, floor: 0.5, speed: 700, width: 30 }, PRESETS['Deep Night'])] },
  { name: 'Multicolor bar', tempo: 'Fast', tags: ['House', 'EDM', 'Pop'],
    desc: 'Every kick sweeps the next color across the strip, back and forth (hits stand in for LedFx’s beat tracking).',
    build: (n) => [part(n, 'Color wipe', 0, 1, { ...hitsOn(LFX_KICK, 2, 10), mode: 'wipe', attack: 0, release: 400, floor: 0.55, speed: 900, width: 100 }, PRESETS['Deep Night'])] },
  { name: 'Bands', tempo: 'Fast', tags: ['Rock', 'Pop', 'Spectrum'],
    desc: 'Six level meters side by side, each for its own slice of the spectrum from bass to treble.',
    build: (n) => bandParts(n, 6, (i) => ({ trigger: 'level', threshold: -50 - 4.4 * i, ceiling: -18 - 4.4 * i, mode: 'fill', attack: 0, release: 150 })) },
  { name: 'Equalizer', tempo: 'Fast', tags: ['Jazz', 'Classical', 'Spectrum'],
    desc: 'Six meters growing from their own centers, with your palette running along the whole strip.',
    build: (n) => bandParts(n, 6, (i) => ({ trigger: 'level', threshold: -50 - 4.4 * i, ceiling: -18 - 4.4 * i, mode: 'fill', attack: 0, release: 150 }), null, { direction: 'center' }) },
  { name: 'Blocks', tempo: 'Fast', tags: ['EDM', 'Spectrum'],
    desc: 'Four color blocks, each showing its quarter of the spectrum LED by LED.',
    build: (n) => bandParts(n, 4, (i) => ({ trigger: 'level', threshold: -68 - 4 * i, ceiling: -28 - 4 * i, mode: 'spectrum', attack: 0, release: 180 }),
      [['#0033CC', '#0033CC'], ['#4B00FF', '#4B00FF'], ['#FF00FF', '#FF00FF'], ['#FF3399', '#FF3399']]) },
  { name: 'Fire', tempo: 'Medium', tags: ['Rock', 'Metal', 'Ambient'],
    desc: 'Violet and pink flames rise up the strip and reach higher with the bass.',
    build: (n) => [part(n, 'Flames', 0, 1, { ...LFX_LOWS, mode: 'fire', attack: 30, release: 400, floor: 0.25, speed: 90 }, NIGHT_FIRE)] },
  { name: 'Lava lamp', tempo: 'Slow', tags: ['Ambient', 'Lo-fi', 'Chill'],
    desc: 'Slow blobs of color drift along the strip; the bass nudges them faster and brighter.',
    build: (n) => [part(n, 'Lava', 0, 1, { ...LFX_LOWS, mode: 'flow', attack: 150, release: 900, floor: 0.4, speed: 12, width: 40 }, PRESETS['Deep Night'])] },
  { name: 'Marching', tempo: 'Medium', tags: ['Hip-hop', 'R&B', 'Chill'],
    desc: 'Narrow bands of color march along the strip and lurch ahead on the beat.',
    build: (n) => [part(n, 'March', 0, 1, { ...LFX_LOWS, mode: 'flow', attack: 0, release: 250, floor: 0.25, speed: 45, width: 8 }, PRESETS['Deep Night'])] },
].map((e) => ({ ...e, source: 'LedFx', tags: ['LedFx', ...e.tags] }));

/* ---------- presets library: genres and tags ---------- */

// Genre presets give each range of the spectrum its own reactive preset.
// A range is a role, a frequency range, the name of a reactive preset (its
// trigger, thresholds and motion), colors, and optional segment overrides.
const UV_PAL = ['#4B00FF', '#8F00FF', '#4B00FF'];
const rng = (role, freq_lo, freq_hi, preset, palette, extra = {}) => ({ role, freq_lo, freq_hi, preset, palette, extra });

// layout: alternate = groups cycling through the ranges, zones = side by side,
// mirror = zones mirrored around the center (last range in the middle),
// stack = every range over the whole strip
const GENRE_PRESETS = [
  { id: 'genre:edm', name: 'EDM festival', tempo: 'Fast', tags: ['EDM', 'House', 'Pop'],
    desc: 'Four-on-the-floor kicks punch, synth leads ripple and hats sparkle in alternating groups.',
    layout: { type: 'alternate', groups: 9 },
    ranges: [rng('Kick', 45, 110, 'Kick punch', LOWS_PAL), rng('Synths', 500, 4000, 'Mid ripple', MIDS_PAL), rng('Hats', 8000, 16000, 'Hi-hat sparkle', HIGHS_PAL)] },
  { id: 'genre:techno', name: 'Techno warehouse', tempo: 'Very fast', tags: ['Techno', 'House', 'Strobe'],
    desc: 'A hard kick strobe in the middle, rumbling mids beside it, cymbal flashes at the ends.',
    layout: { type: 'mirror', weights: [1, 1, 2] },
    ranges: [rng('Hats', 6000, 14000, 'Cymbal flash', HIGHS_PAL), rng('Rumble', 150, 1000, 'Slow breathe', MIDS_PAL), rng('Kick', 40, 100, 'Bass strobe', LOWS_PAL)] },
  { id: 'genre:dnb', name: 'Drum & bass rush', tempo: 'Fast', tags: ['Drum & Bass'],
    desc: 'Snares crack in the center, the reese bass swells around them, breakbeat hats at the ends.',
    layout: { type: 'mirror', weights: [1, 1.5, 1] },
    ranges: [rng('Breaks', 6000, 16000, 'Hi-hat sparkle', HIGHS_PAL), rng('Reese', 40, 90, 'Sub swell', LOWS_PAL), rng('Snare', 1500, 5000, 'Snare snap', MIDS_PAL)] },
  { id: 'genre:dubstep', name: 'Dubstep drop', tempo: 'Fast', tags: ['Dubstep', 'EDM'],
    desc: 'The wobble bass fills out from the middle, snares snap beside it, hats sparkle at the ends.',
    layout: { type: 'mirror', weights: [1, 1, 2] },
    ranges: [rng('Hats', 7000, 16000, 'Hi-hat sparkle', HIGHS_PAL), rng('Snare', 1800, 5000, 'Snare snap', MIDS_PAL), rng('Wobble', 30, 120, 'Bass meter', LOWS_PAL)] },
  { id: 'genre:trap', name: 'Trap 808', tempo: 'Fast', tags: ['Trap', 'Hip-hop'],
    desc: 'Booming 808s with long tails, snappy claps and rolling hi-hats in alternating groups.',
    layout: { type: 'alternate', groups: 9 },
    ranges: [rng('808', 35, 80, 'Heartbeat', LOWS_PAL), rng('Clap', 1500, 5000, 'Snare snap', MIDS_PAL), rng('Hats', 7000, 16000, 'Cymbal flash', HIGHS_PAL)] },
  { id: 'genre:hiphop', name: 'Hip-hop groove', tempo: 'Medium', tags: ['Hip-hop', 'R&B'],
    desc: 'The 808 thumps in the center, vocals glow around it, snares snap at the ends.',
    layout: { type: 'mirror', weights: [1, 1.5, 1.5] },
    ranges: [rng('Snare', 1800, 5000, 'Snare snap', HIGHS_PAL), rng('Vocals', 300, 3000, 'Vocal glow', MIDS_PAL), rng('808', 40, 100, 'Heartbeat', LOWS_PAL)] },
  { id: 'genre:pop', name: 'Pop radio', tempo: 'Fast', tags: ['Pop', 'EDM'],
    desc: 'Kick punches, vocals glow and hats sparkle in alternating groups.',
    layout: { type: 'alternate', groups: 9 },
    ranges: [rng('Kick', 50, 120, 'Kick punch', LOWS_PAL), rng('Vocals', 300, 3000, 'Vocal glow', MIDS_PAL), rng('Hats', 6000, 16000, 'Hi-hat sparkle', HIGHS_PAL)] },
  { id: 'genre:rnb', name: 'R&B slow jam', tempo: 'Medium', tags: ['R&B', 'Hip-hop'],
    desc: 'Warm bass pulses in the center, vocals glow around it, a soft shimmer at the ends.',
    layout: { type: 'mirror', weights: [1, 2, 1.5] },
    ranges: [rng('Air', 6000, 16000, 'Shimmer', HIGHS_PAL), rng('Vocals', 250, 2500, 'Vocal glow', MIDS_PAL), rng('Bass', 40, 150, 'Heartbeat', LOWS_PAL)] },
  { id: 'genre:rock', name: 'Rock stage', tempo: 'Fast', tags: ['Rock'],
    desc: 'Kick, guitars, snare and cymbals each get their own groups along the strip.',
    layout: { type: 'alternate', groups: 8 },
    ranges: [rng('Kick', 60, 120, 'Kick punch', LOWS_PAL), rng('Guitars', 250, 2000, 'Guitar wave', MIDS_PAL), rng('Snare', 1800, 5000, 'Snare snap', UV_PAL), rng('Cymbals', 6000, 14000, 'Cymbal flash', HIGHS_PAL)] },
  { id: 'genre:metal', name: 'Metal blast', tempo: 'Very fast', tags: ['Metal', 'Rock', 'Strobe'],
    desc: 'Double-kick strobes, churning guitars and crashing cymbals in tight groups.',
    layout: { type: 'alternate', groups: 12 },
    ranges: [rng('Double kick', 60, 140, 'Bass strobe', LOWS_PAL), rng('Guitars', 200, 3000, 'Mid ripple', MIDS_PAL), rng('Cymbals', 6000, 16000, 'Cymbal flash', HIGHS_PAL)] },
  { id: 'genre:jazz', name: 'Jazz lounge', tempo: 'Medium', tags: ['Jazz', 'Chill'],
    desc: 'The upright bass as a meter, piano and horns rolling, brushed cymbals shimmering.',
    layout: { type: 'zones', weights: [1, 1.5, 1] },
    ranges: [rng('Upright bass', 40, 250, 'Bass meter', LOWS_PAL, { direction: 'forward' }), rng('Piano & horns', 250, 4000, 'Guitar wave', MIDS_PAL), rng('Brushes', 5000, 14000, 'Shimmer', HIGHS_PAL)] },
  { id: 'genre:classical', name: 'Classical hall', tempo: 'Slow', tags: ['Classical', 'Chill'],
    desc: 'Low strings swell in the center, the orchestra breathes around them, airy shimmer at the ends.',
    layout: { type: 'mirror', weights: [1, 2, 1.5] },
    ranges: [rng('Air', 6000, 16000, 'Shimmer', HIGHS_PAL), rng('Orchestra', 250, 4000, 'Slow breathe', MIDS_PAL), rng('Low strings', 40, 250, 'Sub swell', LOWS_PAL)] },
  { id: 'genre:lofi', name: 'Lo-fi study', tempo: 'Slow', tags: ['Lo-fi', 'Chill', 'Ambient'],
    desc: 'Mellow bass swells, soft keys glow and dusty hats shimmer, gently.',
    layout: { type: 'mirror', weights: [1, 1.5, 1.5] },
    ranges: [rng('Hats', 5000, 12000, 'Shimmer', HIGHS_PAL), rng('Keys', 300, 2000, 'Vocal glow', MIDS_PAL), rng('Bass', 40, 150, 'Sub swell', LOWS_PAL)] },
  { id: 'genre:ambient', name: 'Ambient space', tempo: 'Slow', tags: ['Ambient', 'Chill'],
    desc: 'Colors drift slowly across the whole strip with faint twinkles of air on top.',
    layout: { type: 'stack' },
    ranges: [rng('Drift', 40, 4000, 'Color drift', PRESETS['Deep Night']), rng('Twinkle', 10000, 20000, 'Air twinkle', HIGHS_PAL)] },
];

const GENRE_TAGS = ['EDM', 'House', 'Techno', 'Drum & Bass', 'Dubstep', 'Trap', 'Hip-hop', 'Pop', 'R&B', 'Rock', 'Metal', 'Jazz', 'Classical', 'Lo-fi', 'Ambient'];
// A preset tagged with this shows up under every genre
const ALL_GENRES = 'All genres';
const HIDDEN_TAGS = new Set(['default', ALL_GENRES.toLowerCase()]);

const REACTIVE_BY_NAME = new Map(REACTIVE.flatMap(([, presets]) => presets).map((p) => [p.name, p]));

function rangeSegment(name, start, end, range) {
  const preset = REACTIVE_BY_NAME.get(range.preset);
  return {
    id: newId(), name, enabled: true, solo: false, start, end,
    ...MOTION_DEFAULTS,
    ...preset.response,
    trigger: preset.range.trigger, threshold: preset.range.threshold, ceiling: preset.range.ceiling,
    freq_lo: range.freq_lo, freq_hi: range.freq_hi,
    palette: [...range.palette],
    ...range.extra,
  };
}

function layoutRanges(n, layout, ranges) {
  const span = (from, to) => {
    const start = Math.round(from * n);
    return [start, Math.max(start, Math.round(to * n) - 1)];
  };
  if (layout.type === 'stack') return ranges.map((r) => rangeSegment(r.role, 0, n - 1, r));
  if (layout.type === 'alternate') {
    const counts = {};
    return Array.from({ length: layout.groups }, (_, i) => {
      const r = ranges[i % ranges.length];
      counts[r.role] = (counts[r.role] || 0) + 1;
      const [start, end] = span(i / layout.groups, (i + 1) / layout.groups);
      return rangeSegment(`${r.role} ${counts[r.role]}`, start, end, r);
    });
  }
  const mirror = layout.type === 'mirror';
  const weights = layout.weights || ranges.map(() => 1);
  const order = mirror ? [...ranges, ...ranges.slice(0, -1).reverse()] : ranges;
  const orderWeights = mirror ? [...weights, ...weights.slice(0, -1).reverse()] : weights;
  const total = orderWeights.reduce((a, b) => a + b, 0);
  const middle = ranges.length - 1;
  let pos = 0;
  return order.map((r, i) => {
    const from = pos / total;
    pos += orderWeights[i];
    const [start, end] = span(from, pos / total);
    if (!mirror || i === middle) return rangeSegment(r.role, start, end, r);
    // Mirrored zones flow outwards on the left side too
    const left = i < middle;
    const seg = rangeSegment(`${r.role} ${left ? 'left' : 'right'}`, start, end, r);
    if (left && seg.direction === 'forward') seg.direction = 'reverse';
    return seg;
  });
}

function scaleSegments(segments, from, to) {
  if (from === to) return segments;
  const ratio = to / from;
  return segments.map((s) => {
    const start = Math.min(to - 1, Math.round(s.start * ratio));
    return { ...s, start, end: Math.min(to - 1, Math.max(start, Math.round((s.end + 1) * ratio) - 1)) };
  });
}

function tempoOf(segments) {
  const releases = segments.filter((s) => s.attack < 250).map((s) => s.release);
  if (!releases.length) return 'Slow';
  const fastest = Math.min(...releases);
  if (fastest <= 35) return 'Very fast';
  if (fastest <= 90) return 'Fast';
  return 'Medium';
}

const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
// fx:<name> for EFFECTS, fx:ledfx-<name> for LEDFX_EFFECTS
const effectId = (e) => `fx:${slug(e.source ? `${e.source} ${e.name}` : e.name)}`;
const fresh = (segments) => segments.map((s) => ({ ...s, id: newId(), palette: [...s.palette] }));

// Every preset: saved ones, genre presets, effects. The default comes first.
function library() {
  const saved = (config.presets || []).map((p) => ({
    id: p.id, name: p.name, desc: p.desc, tags: p.tags, saved: true, tempo: tempoOf(p.segments),
    build: (n) => fresh(scaleSegments(p.segments, p.led_count, n)),
  }));
  const genres = GENRE_PRESETS.map((g) => ({ ...g, build: (n) => layoutRanges(n, g.layout, g.ranges) }));
  const effects = [...EFFECTS, ...LEDFX_EFFECTS].map((e) => ({ ...e, id: effectId(e) }));
  const all = [...saved, ...genres, ...effects];
  const index = all.findIndex((p) => p.id === config.default_preset);
  if (index > 0) all.unshift(...all.splice(index, 1));
  return all;
}

// Built segments per preset, for previews and the Active badge
let builtCache = new Map();
function built(preset, n) {
  const key = `${preset.id}|${n}`;
  if (!builtCache.has(key)) builtCache.set(key, preset.build(n));
  return builtCache.get(key);
}

const REACTIVE_ALL = REACTIVE.flatMap(([, presets]) => presets);
const MOTION_REACTION = { scroll: 'Scroll trail', drops: 'Drops', scan: 'Scanner', wipe: 'Color wipe', flow: 'Lava flow', fire: 'Flames' };

// Name of the reactive preset a segment behaves like, or a description
function reactionName(seg) {
  const match = REACTIVE_ALL.find((p) => p.range.trigger === seg.trigger
    && RESPONSE_KEYS.every((k) => sameValue(seg[k], p.response[k])));
  if (match) return match.name;
  if (MOTION_REACTION[seg.mode]) return MOTION_REACTION[seg.mode];
  if (seg.mode === 'shift') return 'Color drift';
  if (seg.attack >= 250) return 'Slow swell';
  if (seg.mode === 'spectrum') return 'Spectrum sparkle';
  if (seg.mode === 'fill') return 'Meter fill';
  if (seg.release <= 60) return 'Fast flicker';
  return 'Pulse';
}

// The frequency ranges of a preset with what drives each of them
function presetRanges(preset) {
  if (preset.ranges) {
    return preset.ranges.map((r) => ({ role: r.role, lo: r.freq_lo, hi: r.freq_hi, reaction: r.preset, palette: r.palette }));
  }
  const groups = new Map();
  for (const s of built(preset, 100)) {
    const key = `${s.freq_lo}|${s.freq_hi}|${reactionName(s)}`;
    if (!groups.has(key)) {
      groups.set(key, {
        role: s.name.replace(/\s+(\d+|left|right)$/i, '') || bandName(s.freq_lo, s.freq_hi),
        lo: s.freq_lo, hi: s.freq_hi, reaction: reactionName(s), palette: s.palette,
      });
    }
  }
  return [...groups.values()];
}

const EFFECT_KEYS = ['start', 'end', 'freq_lo', 'freq_hi', 'trigger', 'threshold', 'ceiling', 'mode', 'direction', 'attack', 'release', 'speed', 'width'];
// Umbrella categories for the effects menu. Saved presets go to My presets;
// built-ins are listed by id (genre:* from GENRE_PRESETS, fx:* from EFFECTS
// and LEDFX_EFFECTS).
const CATEGORIES = [
  { id: 'mine', name: 'My presets', blurb: 'Your saved setups, including the default.' },
  { id: 'ledfx', name: 'LedFx', blurb: 'Audio reactive effects ported from LedFx, in Deep Night colors.',
    items: LEDFX_EFFECTS.map(effectId) },
  { id: 'electronic', name: 'Electronic', blurb: 'EDM, house, techno, drum & bass and dubstep.',
    items: ['genre:edm', 'genre:techno', 'genre:dnb', 'genre:dubstep', 'fx:alternating-flicker', 'fx:strobe-sparkle', 'fx:bass-burst', 'fx:collide'] },
  { id: 'hiphop', name: 'Hip-hop & R&B', blurb: 'Trap, hip-hop, R&B and soul.',
    items: ['genre:trap', 'genre:hiphop', 'genre:rnb'] },
  { id: 'poprock', name: 'Pop & Rock', blurb: 'Pop, rock and metal.',
    items: ['genre:pop', 'genre:rock', 'genre:metal'] },
  { id: 'jazzclassical', name: 'Jazz & Classical', blurb: 'Jazz, acoustic and orchestral.',
    items: ['genre:jazz', 'genre:classical'] },
  { id: 'chill', name: 'Chill & Ambient', blurb: 'Lo-fi, ambient and calm background glows.',
    items: ['genre:lofi', 'genre:ambient', 'fx:starfield', 'fx:color-drift', 'fx:breathing', 'fx:ambient-zones'] },
  { id: 'visualizers', name: 'Visualizers', blurb: 'Meters and spectrum displays for any music.',
    items: ['fx:mirror-spectrum', 'fx:vu-meter', 'fx:equalizer-bands', 'fx:bass-vs-treble'] },
];

let presetItems = [];
let presetGroups = [];
let presetsSignature = '';
let presetQuery = '';
let expandedItem = null;
let editingPreset = null;
let showSaveForm = false;
let openCategories = null;

const presetSig = () => JSON.stringify([config.presets, config.default_preset, config.led_count]);

// Which groups are open is a per-browser convenience
const OPEN_KEY = 'studio.openCategories';
function loadOpenCategories() {
  try {
    const saved = JSON.parse(localStorage.getItem(OPEN_KEY));
    return Array.isArray(saved) ? new Set(saved) : null;
  } catch (err) {
    return null;
  }
}
function saveOpenCategories() {
  try {
    localStorage.setItem(OPEN_KEY, JSON.stringify([...openCategories]));
  } catch (err) {
    // storage unavailable: the menu still works, it just won't remember
  }
}

function presetMatches(p) {
  const segs = built(p, config.led_count);
  return segs.length === config.segments.length
    && segs.every((b, i) => EFFECT_KEYS.every((k) => sameValue(config.segments[i][k], b[k])));
}

// Search over name, description, tags, category and range names
function matchesQuery(p, group, terms) {
  const text = [p.name, p.desc, group.name, ...(p.tags || []),
    ...presetRanges(p).flatMap((r) => [r.role, r.reaction])].join(' ').toLowerCase();
  return terms.every((t) => text.includes(t));
}

// Preview animation for a segment block, from how it reacts
function blockAnimation(seg) {
  const motion = {
    scroll: ['scroll', 1.4], drops: ['sparkle', 0.9], scan: ['scan', 1.6],
    wipe: ['wipe', 1.1], flow: ['drift', 4], fire: ['flicker', 0.9],
  }[seg.mode];
  if (motion) return motion;
  if (seg.mode === 'shift') return ['drift', 6];
  if (seg.attack >= 250) return ['breathe', Math.max(2, (seg.attack + seg.release) / 1000)];
  if (seg.mode === 'spectrum') return ['sparkle', 0.5];
  if (seg.mode === 'fill') return ['fill', 0.9];
  if (seg.release <= 60) return ['kick', seg.release <= 40 ? 0.25 : 0.4];
  return ['kick', 0.9];
}

function presetPreview(preset, variant = '') {
  const segs = built(preset, 100);
  const sorted = [...segs].sort((a, b) => a.start - b.start);
  const ends = [];
  const lane = new Map();
  for (const s of sorted) {
    let l = ends.findIndex((e) => e < s.start);
    if (l === -1) { l = ends.length; ends.push(s.end); } else ends[l] = s.end;
    lane.set(s, l);
  }
  const lanes = Math.max(1, ends.length);
  const blocks = segs.map((s, i) => {
    const [anim, dur] = blockAnimation(s);
    const origin = s.direction === 'reverse' ? 'right' : s.direction === 'forward' ? 'left' : 'center';
    return h('i', {
      class: `fx-block fx-${anim}`,
      style: `left:${s.start}%;width:${s.end - s.start + 1}%;top:${(lane.get(s) / lanes) * 100}%;height:${100 / lanes}%;`
        + `background:${gradientCss(s.palette)};--dur:${dur}s;animation-delay:${-(i * 0.17).toFixed(2)}s;transform-origin:${origin}`,
    });
  });
  return h('span', { class: `fx-preview ${variant}`.trim(), 'aria-hidden': 'true' }, blocks);
}

// Name and tags form, for saving the current setup and editing saved presets
function presetForm({ title, name, tags, saveLabel, onSave, onCancel, onDelete }) {
  const chosen = new Set(tags.filter((t) => GENRE_TAGS.includes(t) || t === ALL_GENRES));
  const nameInput = h('input', { type: 'text', maxlength: '40', value: name, 'aria-label': 'Preset name' });
  const genreChips = [...GENRE_TAGS, ALL_GENRES].map((tag) => {
    const chip = h('button', { type: 'button', class: 'chip-btn', 'aria-pressed': String(chosen.has(tag)) }, tag);
    chip.addEventListener('click', () => {
      if (chosen.has(tag)) chosen.delete(tag); else chosen.add(tag);
      chip.setAttribute('aria-pressed', String(chosen.has(tag)));
    });
    return chip;
  });
  const other = tags.filter((t) => !GENRE_TAGS.includes(t) && t !== ALL_GENRES);
  const customInput = h('input', { type: 'text', value: other.join(', '), placeholder: 'e.g. Party, Gaming, Late night', 'aria-label': 'More tags, comma separated' });
  const save = h('button', { type: 'button', class: 'btn small primary' }, saveLabel);
  save.addEventListener('click', () => {
    const custom = customInput.value.split(',').map((t) => t.trim()).filter(Boolean);
    const seen = new Set();
    const all = [...GENRE_TAGS, ALL_GENRES].filter((t) => chosen.has(t)).concat(custom)
      .filter((t) => !seen.has(t.toLowerCase()) && seen.add(t.toLowerCase()));
    onSave(nameInput.value.trim() || 'My preset', all);
  });
  const cancel = h('button', { type: 'button', class: 'btn small ghost' }, 'Cancel');
  cancel.addEventListener('click', onCancel);
  const buttons = [save, cancel];
  if (onDelete) {
    const del = h('button', { type: 'button', class: 'btn small ghost danger push' }, 'Delete');
    confirmClick(del, 'Confirm delete', onDelete);
    buttons.push(del);
  }
  const form = h('div', { class: 'preset-form' }, [
    h('strong', null, title),
    h('label', { class: 'pf-field' }, [h('span', null, 'Name'), nameInput]),
    h('div', { class: 'pf-field' }, [h('span', null, 'Genres'), h('div', { class: 'band-chips' }, genreChips)]),
    h('label', { class: 'pf-field' }, [h('span', null, 'More tags'), customInput]),
    h('div', { class: 'pf-buttons' }, buttons),
  ]);
  setTimeout(() => nameInput.focus(), 0);
  return form;
}

function renderPresets() {
  if (!config) return;
  presetsSignature = presetSig();
  builtCache = new Map();
  const lib = library();
  const byId = new Map(lib.map((p) => [p.id, p]));

  // Save form
  const formRoot = $('#preset-form');
  formRoot.replaceChildren();
  if (showSaveForm) {
    formRoot.append(presetForm({
      title: 'Save the current segments', name: '', tags: [], saveLabel: 'Save preset',
      onSave: (name, tags) => {
        const preset = { id: newId(), name, desc: '', tags, led_count: config.led_count, segments: JSON.parse(JSON.stringify(config.segments)) };
        config.presets = [...(config.presets || []), preset];
        showSaveForm = false;
        if (openCategories) openCategories.add('mine');
        commit();
        renderPresets();
        toast(`Saved ${name} to My presets`);
      },
      onCancel: () => { showSaveForm = false; renderPresets(); },
    }));
  }

  // Group the library under the umbrella categories, the default first
  const first = (list) => [...list].sort((a, b) => (b.id === config.default_preset) - (a.id === config.default_preset));
  const groups = CATEGORIES.map((c) => ({
    ...c,
    presets: first(c.id === 'mine' ? lib.filter((p) => p.saved) : c.items.map((id) => byId.get(id)).filter(Boolean)),
  }));
  const listed = new Set(groups.flatMap((g) => g.presets.map((p) => p.id)));
  const rest = lib.filter((p) => !listed.has(p.id));
  if (rest.length) groups.push({ id: 'more', name: 'More', blurb: '', presets: rest });

  if (!openCategories) {
    openCategories = loadOpenCategories()
      || new Set(['mine', ...groups.filter((g) => g.presets.some(presetMatches)).map((g) => g.id)]);
  }

  const terms = presetQuery.toLowerCase().split(/\s+/).filter(Boolean);
  presetItems = [];
  presetGroups = [];
  const sections = [];
  for (const g of groups) {
    const shown = terms.length ? g.presets.filter((p) => matchesQuery(p, g, terms)) : g.presets;
    if (!shown.length && (terms.length || g.id !== 'mine')) continue;
    const details = h('details', { class: 'menu-group', open: terms.length > 0 || openCategories.has(g.id) });
    details.append(h('summary', null, [
      h('span', { class: 'mg-name' }, g.name),
      h('span', { class: 'mg-count' }, String(shown.length)),
    ]));
    if (g.blurb) details.append(h('p', { class: 'mg-blurb' }, g.blurb));
    details.append(h('ul', { class: 'menu-items' }, shown.length
      ? shown.map(menuItem)
      : h('li', { class: 'menu-empty' }, 'Nothing saved yet. Use Save current to keep your setup here.')));
    details.addEventListener('toggle', () => {
      if (terms.length) return;
      if (details.open) openCategories.add(g.id); else openCategories.delete(g.id);
      saveOpenCategories();
    });
    presetGroups.push([details, shown]);
    sections.push(details);
  }
  if (!sections.length) sections.push(h('p', { class: 'empty-list' }, `No effects match “${presetQuery}”.`));
  $('#preset-menu').replaceChildren(...sections);
  syncPresets();
}

function menuItem(p) {
  const isDefault = p.id === config.default_preset;
  const expanded = expandedItem === p.id;
  const apply = h('button', {
    type: 'button', class: 'mi-apply',
    'aria-label': `Apply ${p.name}${p.source ? ` (${p.source})` : ''}, ${p.tempo}${isDefault ? ', default' : ''}`,
  }, [
    presetPreview(p, 'mini'),
    h('span', { class: 'mi-name' }, p.name),
    p.source ? h('span', { class: 'mi-badge', title: `Ported from ${p.source}`, 'aria-hidden': 'true' }, p.source) : null,
    isDefault ? h('span', { class: 'mi-star', 'aria-hidden': 'true' }, '★') : null,
    h('span', { class: `mi-tempo ${TEMPO_CLASS[p.tempo]}`, title: p.tempo, 'aria-hidden': 'true' }),
  ]);
  apply.addEventListener('click', () => applyPreset(p));
  const more = h('button', {
    type: 'button', class: 'mi-more', 'aria-expanded': String(expanded),
    'aria-label': `Details for ${p.name}`, title: 'Details',
  }, '▾');
  more.addEventListener('click', () => {
    expandedItem = expanded ? null : p.id;
    if (!expanded) editingPreset = null;
    renderPresets();
  });
  const item = h('li', { class: `menu-item${expanded ? ' expanded' : ''}` }, h('div', { class: 'mi-row' }, [apply, more]));
  if (expanded) item.append(presetDetails(p, isDefault));
  presetItems.push([p, item, apply]);
  return item;
}

function presetDetails(p, isDefault) {
  const ranges = presetRanges(p);
  const tags = (p.tags || []).filter((t) => !HIDDEN_TAGS.has(t.toLowerCase()));

  const star = h('button', {
    type: 'button', class: 'btn small ghost star', 'aria-pressed': String(isDefault),
    title: isDefault ? 'This is the default: Reset all restores it' : 'Make this the default that Reset all restores',
  }, isDefault ? '★ Default' : '☆ Make default');
  star.addEventListener('click', () => {
    config.default_preset = isDefault ? '' : p.id;
    commit();
    renderPresets();
    toast(isDefault ? 'No default preset: Reset all restores the basic three segments' : `${p.name} is now the default`);
  });
  const actions = [star];
  if (p.saved) {
    const edit = h('button', { type: 'button', class: 'btn small ghost', 'aria-expanded': String(editingPreset === p.id) }, 'Edit');
    edit.addEventListener('click', () => {
      editingPreset = editingPreset === p.id ? null : p.id;
      renderPresets();
    });
    actions.push(edit);
  }

  const details = h('div', { class: 'mi-details' }, [
    p.desc ? h('small', null, p.desc) : null,
    h('ul', { class: 'fx-ranges' }, ranges.map((r) => h('li', null, [
      h('i', { style: `background:${gradientCss(r.palette)}` }),
      h('b', null, r.role),
      h('span', { class: 'mono' }, `${fmtHz(r.lo)}–${fmtHz(r.hi)}`),
      h('span', { class: 'fx-reaction' }, r.reaction),
    ]))),
    tags.length ? h('div', { class: 'fx-tags' }, tags.map((t) => h('span', { class: 'tag' }, t))) : null,
    h('div', { class: 'fx-actions' }, actions),
  ]);

  if (p.saved && editingPreset === p.id) {
    const stored = config.presets.find((x) => x.id === p.id);
    details.append(presetForm({
      title: 'Edit preset', name: stored.name, tags: stored.tags, saveLabel: 'Save changes',
      onSave: (name, newTags) => {
        stored.name = name;
        stored.tags = newTags;
        editingPreset = null;
        commit();
        renderPresets();
      },
      onCancel: () => { editingPreset = null; renderPresets(); },
      onDelete: () => {
        config.presets = config.presets.filter((x) => x.id !== p.id);
        if (config.default_preset === p.id) config.default_preset = '';
        editingPreset = null;
        expandedItem = null;
        commit();
        renderPresets();
        toast(`Deleted ${p.name}`);
      },
    }));
  }
  return details;
}

// Marks the effects the current segments match, re-renders if presets changed
function syncPresets() {
  if (!config) return;
  if (presetSig() !== presetsSignature) {
    renderPresets();
    return;
  }
  const active = new Set();
  for (const [p, item, apply] of presetItems) {
    const match = presetMatches(p);
    item.classList.toggle('is-active', match);
    if (match) {
      apply.setAttribute('aria-current', 'true');
      active.add(p.id);
    } else {
      apply.removeAttribute('aria-current');
    }
  }
  for (const [details, shown] of presetGroups) {
    details.classList.toggle('has-active', shown.some((p) => active.has(p.id)));
  }
}

function replaceSegments(segments, message, extra = {}) {
  const previous = { segments: JSON.parse(JSON.stringify(config.segments)), master: config.master, background: config.background };
  const previousSelected = selectedId;
  Object.assign(config, extra);
  config.segments = segments;
  selectedId = config.segments.length ? config.segments[0].id : null;
  renderList();
  renderInspector();
  renderSettings();
  commit();
  toast(message, {
    label: 'Undo',
    run: () => {
      Object.assign(config, previous);
      selectedId = previous.segments.some((s) => s.id === previousSelected) ? previousSelected : (previous.segments[0] ? previous.segments[0].id : null);
      renderList();
      renderInspector();
      renderSettings();
      commit();
      toast('Your previous setup is back');
    },
  });
}

function applyPreset(preset) {
  replaceSegments(preset.build(config.led_count), `${preset.source ? `${preset.source} ` : ''}${preset.name} is on`);
}

async function resetAll() {
  // With a default preset: restore it locally (built-ins only exist here)
  const def = library().find((p) => p.id === config.default_preset);
  if (def) {
    replaceSegments(def.build(config.led_count), `Reset to your default preset, ${def.name}`, { master: 1, background: '#000000' });
    return;
  }
  // Drop pending edits so they can't overwrite the reset
  clearTimeout(saveTimer);
  pendingSave = false;
  editSeq++;
  saving++;
  try {
    config = await api('/api/reset', {});
  } catch (err) {
    toast(`Reset failed: ${err.message}`);
    return;
  } finally {
    saving--;
  }
  selectedId = config.segments.length ? config.segments[0].id : null;
  renderList();
  renderInspector();
  renderSettings();
  toast('Reset to the default segments');
}

/* ---------- colors by spectrum range ---------- */

// Segments are grouped by the middle (geometric mean) of their frequency range
const COLOR_RANGES = [
  ['All segments', 0, Infinity],
  ['Sub & bass', 0, 250],
  ['Low mids', 250, 500],
  ['Mids', 500, 2000],
  ['High mids', 2000, 6000],
  ['Highs', 6000, Infinity],
];
let colorsSignature = '';
let colorsDirty = false;

const centerHz = (s) => Math.sqrt(s.freq_lo * s.freq_hi);
const segmentsIn = (lo, hi) => config.segments.filter((s) => centerHz(s) >= lo && centerHz(s) < hi);
const colorsSig = () => JSON.stringify(config.segments.map((s) => [s.id, s.freq_lo, s.freq_hi, s.palette]));

function rangeHz(lo, hi) {
  if (hi === Infinity) return lo ? `above ${fmtHz(lo)}` : 'every range';
  return lo ? `${fmtHz(lo)}–${fmtHz(hi)}` : `below ${fmtHz(hi)}`;
}

function colorRangeRow(label, lo, hi) {
  const segs = segmentsIn(lo, hi);
  const head = h('div', { class: 'cr-head' }, [
    h('strong', null, label),
    h('small', null, `${rangeHz(lo, hi)} · ${segs.length} segment${segs.length === 1 ? '' : 's'}`),
  ]);
  if (!segs.length) return h('div', { class: 'cr-row empty' }, head);

  const palette = [...segs[0].palette];
  const mixed = segs.some((s) => s.palette.join() !== segs[0].palette.join());
  const applyAll = () => {
    for (const s of segmentsIn(lo, hi)) {
      s.palette = [...palette];
      updateListRow(s);
    }
    syncInspector();
    commit();
  };

  const chips = palette.map((color, i) => {
    const chip = h('div', { class: 'chip', style: `background:${color}` });
    const input = h('input', { type: 'color', value: color.toLowerCase(), 'aria-label': `${label}: color ${i + 1} of ${palette.length}` });
    input.addEventListener('input', () => {
      palette[i] = input.value.toUpperCase();
      chip.style.background = input.value;
      applyAll();
    });
    chip.append(input);
    if (palette.length > 1) {
      const remove = h('button', { type: 'button', class: 'chip-x', 'aria-label': `${label}: remove color ${i + 1}`, title: 'Remove' }, '×');
      remove.addEventListener('click', () => {
        palette.splice(i, 1);
        applyAll();
        renderColorRanges(true);
      });
      chip.append(remove);
    }
    return chip;
  });
  const add = h('button', { type: 'button', class: 'chip-add', 'aria-label': `${label}: add color`, title: 'Add color' }, '+');
  add.disabled = palette.length >= 16;
  add.addEventListener('click', () => {
    palette.push(palette[palette.length - 1]);
    applyAll();
    renderColorRanges(true);
  });

  const pick = h('select', { 'aria-label': `${label}: use a palette` }, [
    h('option', { value: '' }, 'Use a palette…'),
    ...Object.keys(PRESETS).map((name) => h('option', { value: name }, name)),
  ]);
  pick.addEventListener('change', () => {
    if (!pick.value) return;
    palette.splice(0, palette.length, ...PRESETS[pick.value]);
    applyAll();
    pick.blur();
    renderColorRanges(true);
  });
  const reverse = h('button', { type: 'button', class: 'btn small ghost', title: 'Reverse the colors' }, '⇄');
  reverse.setAttribute('aria-label', `${label}: reverse colors`);
  reverse.addEventListener('click', () => {
    palette.reverse();
    applyAll();
    renderColorRanges(true);
  });

  return h('div', { class: `cr-row${label === 'All segments' ? ' all' : ''}` }, [
    head,
    h('div', { class: 'cr-controls' }, [h('div', { class: 'chips' }, [...chips, add]), pick, reverse]),
    mixed ? h('p', { class: 'note' }, 'These segments have different colors. Changing them here gives them all the colors shown.') : null,
  ]);
}

// Rebuilding would close an open color picker, so wait until focus leaves
function renderColorRanges(force = false) {
  const root = $('#color-ranges');
  if (!config) return;
  if (!force && root.contains(document.activeElement)) {
    colorsDirty = true;
    return;
  }
  colorsDirty = false;
  colorsSignature = colorsSig();
  root.replaceChildren(...COLOR_RANGES.map(([label, lo, hi]) => colorRangeRow(label, lo, hi)));
}

function syncColorRanges() {
  if (config && colorsSig() !== colorsSignature) renderColorRanges();
}

function setupColorRanges() {
  const root = $('#color-ranges');
  root.addEventListener('focusout', (e) => {
    if (colorsDirty && !root.contains(e.relatedTarget)) renderColorRanges();
  });
}

/* ---------- collapsible cards ---------- */

// Which cards are collapsed is a per-browser convenience, like open menu groups
const COLLAPSED_KEY = 'studio.collapsedCards';
function setupCollapsibles() {
  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(COLLAPSED_KEY));
  } catch (err) {
    // storage unavailable: cards start in their default state
  }
  const state = saved && typeof saved === 'object' ? saved : {};
  for (const card of document.querySelectorAll('[data-collapse]')) {
    const key = card.dataset.collapse;
    const title = card.querySelector(':scope > .card-head > h2, :scope > .menu-head > h2');
    const button = h('button', { type: 'button', class: 'collapse-btn' }, title.textContent);
    title.replaceChildren(button);
    const set = (collapsed) => {
      card.classList.toggle('collapsed', collapsed);
      button.setAttribute('aria-expanded', String(!collapsed));
    };
    set(key in state ? Boolean(state[key]) : card.hasAttribute('data-collapsed'));
    button.addEventListener('click', () => {
      const collapsed = !card.classList.contains('collapsed');
      set(collapsed);
      state[key] = collapsed;
      try {
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify(state));
      } catch (err) {
        // storage unavailable: the card still toggles, it just won't remember
      }
    });
  }
}

/* ---------- settings, status ---------- */

function renderSettings() {
  $('#set-host').value = config.hyperion.host;
  $('#set-port').value = config.hyperion.port;
  $('#set-priority').value = config.hyperion.priority;
  $('#set-fps').value = config.hyperion.fps;
  $('#set-bg').value = config.background.toLowerCase();
  $('#set-leds').textContent = `${config.led_count} LEDs · follows Hyperion`;
  syncBrightness();
  const out = $('#output-toggle');
  out.setAttribute('aria-pressed', String(config.hyperion.enabled));
  out.textContent = config.hyperion.enabled ? 'Output on' : 'Output off';
  syncAmbient();
  syncAudioInput();
}

/* ---------- audio reactive and ambient light ---------- */

// config.reactive: on (segments follow the audio), auto (ambient light after
// config.ambient.idle_after seconds of silence) or off (ambient light only).
// The engine fades between the two (studio_engine.Ambient).
const REACTIVE_MODES = [
  ['on', 'On', 'The segments follow the music'],
  ['auto', 'Auto', 'Follows the music, and fades to the ambient light once it has been quiet for a while'],
  ['off', 'Off', 'Shows the ambient light and ignores the audio'],
];
const AMBIENT_COLORS = [
  ['#0033CC', 'Deep blue'], ['#000080', 'Navy'], ['#4B00FF', 'UV'], ['#FF3399', 'Pink'],
  ['#FF00FF', 'Magenta'], ['#800080', 'Purple'], ['#8F00FF', 'Violet'], ['#4B0082', 'Indigo'],
];
const AMBIENT_STYLES = [
  ['solid', 'Solid', 'A steady color'],
  ['breathe', 'Breathe', 'Slowly brightens and dims, one breath every 8 seconds'],
];
// Positions of the "switch after" slider, in seconds
const IDLE_STEPS = [1, 2, 3, 5, 8, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300, 600];
const fmtSeconds = (s) => (s < 60 ? `${s} s` : `${+(s / 60).toFixed(1)} min`);

let ambientRefs = null;

function setReactive(mode) {
  config.reactive = mode;
  syncAmbient();
  commit();
}

function setAmbient(key, value) {
  config.ambient[key] = value;
  syncAmbient();
  commit();
}

function reactiveButtons(container) {
  const buttons = REACTIVE_MODES.map(([mode, text, title]) => {
    const button = h('button', { type: 'button', title, 'data-mode': mode }, text);
    button.addEventListener('click', () => setReactive(mode));
    return button;
  });
  container.replaceChildren(...buttons);
  return buttons;
}

function renderAmbient() {
  const swatches = AMBIENT_COLORS.map(([color, name]) => {
    const button = h('button', { type: 'button', class: 'amb-swatch', style: `background:${color}`, title: name, 'aria-label': name, 'data-color': color });
    button.addEventListener('click', () => setAmbient('color', color));
    return button;
  });
  const custom = h('input', { type: 'color', 'aria-label': 'Custom color', title: 'Custom color' });
  custom.addEventListener('input', () => setAmbient('color', custom.value.toUpperCase()));
  const styles = AMBIENT_STYLES.map(([style, text, title]) => {
    const button = h('button', { type: 'button', title }, text);
    button.addEventListener('click', () => setAmbient('style', style));
    return [style, button];
  });
  const level = h('input', { type: 'range', min: 0, max: 100, step: 1, 'aria-label': 'Ambient brightness' });
  const levelOut = h('output', { class: 'mono' });
  level.addEventListener('input', () => {
    setAmbient('level', Math.pow(Number(level.value) / 100, BRIGHTNESS_GAMMA));
  });
  const idle = h('input', { type: 'range', min: 0, max: IDLE_STEPS.length - 1, step: 1, 'aria-label': 'Switch to the ambient light after this much silence' });
  const idleOut = h('output', { class: 'mono' });
  idle.addEventListener('input', () => setAmbient('idle_after', IDLE_STEPS[Number(idle.value)]));
  const modeHost = h('div', { class: 'seg-ctl', role: 'group', 'aria-label': 'Audio reactive' });
  const now = h('p', { class: 'amb-now', role: 'status' }, [h('i'), h('span')]);
  const idleRow = h('div', { class: 'field' }, [h('label', null, 'Switch after'), h('div', { class: 'ctl' }, [idle, idleOut])]);

  $('#ambient-body').replaceChildren(
    h('div', { class: 'field' }, [h('label', null, 'Audio reactive'), modeHost]),
    now,
    h('div', { class: 'field' }, [h('label', null, 'Color'), h('div', { class: 'amb-swatches' }, [...swatches, custom])]),
    h('div', { class: 'field' }, [h('label', null, 'Style'), h('div', { class: 'seg-ctl', role: 'group', 'aria-label': 'Style' }, styles.map(([, b]) => b))]),
    h('div', { class: 'field' }, [h('label', null, 'Brightness'), h('div', { class: 'ctl' }, [level, levelOut])]),
    idleRow,
  );
  ambientRefs = {
    modes: [...reactiveButtons(modeHost), ...reactiveButtons($('#reactive-top'))],
    swatches, custom, styles, level, levelOut, idle, idleOut, idleRow, now, lastNow: '',
  };
  $('#ambient-dot').addEventListener('click', () => {
    const card = $('#ambient');
    if (card.classList.contains('collapsed')) card.querySelector('.collapse-btn').click();
    card.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  });
  syncAmbient();
}

function syncAmbient() {
  const r = ambientRefs;
  if (!r) return;
  const a = config.ambient;
  for (const button of r.modes) button.setAttribute('aria-pressed', String(button.dataset.mode === config.reactive));
  for (const button of r.swatches) button.setAttribute('aria-pressed', String(button.dataset.color === a.color));
  if (document.activeElement !== r.custom) r.custom.value = a.color.toLowerCase();
  for (const [style, button] of r.styles) button.setAttribute('aria-pressed', String(style === a.style));
  const percent = Math.round(Math.pow(a.level, 1 / BRIGHTNESS_GAMMA) * 100);
  if (document.activeElement !== r.level) r.level.value = percent;
  r.levelOut.textContent = `${percent}%`;
  const step = IDLE_STEPS.reduce((best, s, i) => (Math.abs(s - a.idle_after) < Math.abs(IDLE_STEPS[best] - a.idle_after) ? i : best), 0);
  if (document.activeElement !== r.idle) r.idle.value = step;
  r.idleOut.textContent = fmtSeconds(a.idle_after);
  r.idleRow.hidden = config.reactive !== 'auto';
  // Sliders and buttons in the card take the ambient color
  $('#ambient').style.setProperty('--seg', a.color);
  $('#ambient-dot').style.setProperty('--amb', a.color);
  $('#ambient-dot').title = `Ambient light: ${AMBIENT_COLORS.find(([c]) => c === a.color)?.[1] || a.color}, ${a.style}. Click for its settings.`;
  updateAmbientStatus();
}

// What the strip shows right now, from the engine's fade (status.ambient)
function updateAmbientStatus() {
  const r = ambientRefs;
  if (!r) return;
  const status = online && live.status;
  const amount = status ? status.ambient || 0 : 0;
  const quiet = status ? status.quiet || 0 : 0;
  let state = 'reactive';
  let text;
  if (!status) {
    text = 'The studio is offline';
  } else if (amount >= 0.99) {
    state = 'ambient';
    text = config.reactive === 'off' ? 'Showing the ambient light' : `Showing the ambient light, quiet for ${fmtSeconds(Math.floor(quiet))}`;
  } else if (amount > 0.01) {
    state = 'fading';
    text = 'Fading…';
  } else if (config.reactive === 'auto' && quiet >= 1) {
    const left = Math.max(0, Math.ceil(config.ambient.idle_after - quiet));
    text = `Quiet for ${fmtSeconds(Math.floor(quiet))}, ambient light in ${fmtSeconds(left)}`;
  } else {
    text = quiet >= 1 ? 'Showing the segments, no sound' : 'Showing the segments, following the music';
  }
  const key = `${state}|${text}`;
  if (key === r.lastNow) return;
  r.lastNow = key;
  r.now.className = `amb-now ${state}`;
  r.now.lastChild.textContent = text;
  $('#ambient-dot').classList.toggle('on', state !== 'reactive');
}

// The brightness sliders are perceptual: LEDs look far brighter than their
// output level, so the slider position p maps to an output scale of p^2.2 and
// each step looks about equally large. config.master is the output scale.
const BRIGHTNESS_GAMMA = 2.2;
const BRIGHTNESS_SLIDERS = ['#master', '#brightness'];

const brightnessPercent = () => Math.round(Math.pow(config.master, 1 / BRIGHTNESS_GAMMA) * 100);

function setBrightness(percent) {
  config.master = Math.pow(clamp(percent, 0, 100) / 100, BRIGHTNESS_GAMMA);
  syncBrightness(percent);
  commit();
}

function syncBrightness(percent = brightnessPercent()) {
  for (const id of BRIGHTNESS_SLIDERS) {
    const slider = $(id);
    if (document.activeElement !== slider) slider.value = percent;
  }
  $('#master-out').textContent = `${percent}%`;
  $('#brightness-out').textContent = `${percent}%`;
  $('#brightness').style.setProperty('--pct', `${percent}%`);
}

// Updates everything from `config` without rebuilding focused inputs
function refreshValues() {
  if (listRows.size !== config.segments.length || config.segments.some((s) => !listRows.has(s.id))) renderList();
  else for (const seg of config.segments) updateListRow(seg);
  syncInspector();
  if (!document.querySelector('.settings').contains(document.activeElement)) renderSettings();
}

function applyState(state) {
  config = state.config;
  devices = state.devices || [];
  defaultDevice = state.default_device || '';
  apps = state.apps || [];
  display = state.display || display;
  centers = Array.from({ length: display.bands }, (_, i) => display.min_hz * Math.pow(display.max_hz / display.min_hz, (i + 0.5) / display.bands));
  if (!selected()) selectedId = config.segments.length ? config.segments[0].id : null;
  renderList();
  renderInspector();
  renderSettings();
  invalidate();
}

function setupControls() {
  $('#add-seg').addEventListener('click', () => addSegment());
  confirmClick($('#auto-split'), 'Replace all?', () => autoSplit(Number($('#split-n').value)),
    () => config.segments.length > 0);
  confirmClick($('#reset-all'), 'Confirm reset', resetAll);
  $('#save-preset').addEventListener('click', () => {
    showSaveForm = !showSaveForm;
    renderPresets();
  });
  $('#preset-search').addEventListener('input', (e) => {
    presetQuery = e.target.value.trim();
    renderPresets();
  });

  // The effects menu sticks below the top bar, whose height depends on wrapping
  const topbar = $('.topbar');
  const setTopbarHeight = () => document.documentElement.style.setProperty('--topbar-h', `${topbar.offsetHeight}px`);
  new ResizeObserver(setTopbarHeight).observe(topbar);
  setTopbarHeight();
  confirmClick($('#clear-all'), 'Confirm clear', () => {
    config.segments = [];
    selectedId = null;
    renderList();
    renderInspector();
    commit();
  });

  for (const id of BRIGHTNESS_SLIDERS) {
    $(id).addEventListener('input', (e) => setBrightness(Number(e.target.value)));
  }
  $('#output-toggle').addEventListener('click', () => {
    config.hyperion.enabled = !config.hyperion.enabled;
    renderSettings();
    commit();
  });

  $('#set-host').addEventListener('change', (e) => { config.hyperion.host = e.target.value.trim() || 'localhost'; commit(); });
  for (const [id, key] of [['#set-port', 'port'], ['#set-priority', 'priority'], ['#set-fps', 'fps']]) {
    $(id).addEventListener('change', (e) => {
      const v = Number(e.target.value);
      if (Number.isFinite(v)) config.hyperion[key] = Math.round(v);
      commit();
    });
  }
  $('#set-bg').addEventListener('input', (e) => { config.background = e.target.value.toUpperCase(); commit(); });
  $('#rescan').addEventListener('click', async () => {
    try {
      await refreshSources();
      const playing = apps.filter((a) => a.active).length;
      toast(`Found ${devices.length} audio devices and ${apps.length} apps with audio${playing ? `, ${playing} playing` : ''}`);
    } catch (err) {
      toast(`Rescan failed: ${err.message}`);
    }
  });
  $('#reload-layout').addEventListener('click', async () => {
    try {
      const state = await api('/api/layout', {});
      applyState(state);
      toast(state.layout_ok ? `Loaded ${state.config.led_count} LEDs from Hyperion` : 'Could not reach Hyperion, kept the current LED count');
    } catch (err) {
      toast(`Reload failed: ${err.message}`);
    }
  });

  document.addEventListener('keydown', (e) => {
    const t = e.target;
    const typing = t.closest && t.closest('input, select, textarea, [contenteditable]');
    if (typing) return;
    if (e.key === 'Delete' && selected()) {
      e.preventDefault();
      deleteSelected();
    } else if (e.key === 'Escape') {
      select(null);
    } else if ((e.key === 'd' || e.key === 'D') && (e.ctrlKey || e.metaKey) && selected()) {
      e.preventDefault();
      duplicateSelected();
    } else if (e.key === '[' || e.key === ']') {
      setBrightness(brightnessPercent() + (e.key === ']' ? 5 : -5));
    }
  });
}

function setPill(el, state, text, title) {
  const key = `${state}|${text}|${title}`;
  if (el.dataset.key === key) return;
  el.dataset.key = key;
  el.className = `pill ${state}`;
  el.querySelector('span').textContent = text;
  el.title = title;
}

function updateStatus() {
  const audioEl = $('#st-audio');
  const hypEl = $('#st-hyperion');
  const fpsEl = $('#st-fps');
  const status = live.status;
  updateAmbientStatus();
  updateAudioInput();
  if (!online || !status) {
    setPill(audioEl, 'err', 'Studio offline', 'The studio server is not running');
    setPill(hypEl, '', 'Hyperion', '');
    if (fpsEl.textContent !== '– fps') fpsEl.textContent = '– fps';
    return;
  }
  const a = status.audio;
  if (a.ok) setPill(audioEl, 'ok', a.device, `Capturing: ${a.device}`);
  else setPill(audioEl, 'err', 'No audio', a.message || 'Audio capture failed');
  const hs = status.hyperion;
  if (hs === 'connected') setPill(hypEl, 'ok', 'Hyperion', `Sending to ${config.hyperion.host}:${config.hyperion.port}, priority ${config.hyperion.priority}`);
  else if (hs === 'off') setPill(hypEl, '', 'Output off', 'Not sending to Hyperion');
  else setPill(hypEl, 'warn', 'Hyperion offline', `Can't reach ${config.hyperion.host}:${config.hyperion.port}, retrying`);
  const fps = `${status.fps} fps`;
  if (fpsEl.textContent !== fps) fpsEl.textContent = fps;
}

/* ---------- audio input ---------- */

// config.audio picks what the studio listens to: an output's loopback
// (device: 'default' or a name), an input (device: 'in:' + name), or an app
// (app: exe name, app_mode: 'only' it or everything 'except' it). The select
// encodes all of them as 'out:', 'in:', 'only:' and 'except:' values.
const METER_MIN_DB = -60;
const SOURCE_REFRESH_MS = 5000;
let audioRefs = null;

function sourceValue() {
  const a = config.audio;
  if (a.app) return `${a.app_mode}:${a.app}`;
  if (a.device.startsWith('in:')) return a.device;
  return `out:${a.device}`;
}

function setSource(value) {
  const cut = value.indexOf(':');
  const kind = value.slice(0, cut);
  const rest = value.slice(cut + 1);
  if (kind === 'only' || kind === 'except') {
    config.audio.app = rest;
    config.audio.app_mode = kind;
  } else {
    config.audio.app = '';
    config.audio.device = kind === 'in' ? value : rest;
  }
  syncAudioInput();
  commit();
}

const appLabel = (exe) => {
  const app = apps.find((a) => a.exe === exe);
  return app ? app.name : exe.replace(/\.exe$/i, '');
};

function renderSourceOptions() {
  const r = audioRefs;
  if (!r || document.activeElement === r.select) return;
  const outputs = devices.filter((d) => d.output);
  const inputs = devices.filter((d) => !d.output);
  const opt = (value, text) => h('option', { value }, text);
  const group = (label, options) => (options.length ? h('optgroup', { label }, options) : null);
  const groups = [
    group('Everything you hear', [
      opt('out:default', `Default output${defaultDevice ? ` · ${defaultDevice}` : ''}`),
      ...outputs.map((d) => opt(`out:${d.name}`, d.name)),
    ]),
    group('Only one app', apps.map((a) => opt(`only:${a.exe}`, `${a.name}${a.active ? ' · playing' : ''}`))),
    group('Everything except one app', apps.map((a) => opt(`except:${a.exe}`, `Everything except ${a.name}`))),
    group('Inputs', inputs.map((d) => opt(`in:${d.name}`, d.name))),
  ].filter(Boolean);
  // Keep the saved source listed while its app isn't running or its device is unplugged
  const current = sourceValue();
  const listed = groups.some((g) => [...g.children].some((o) => o.value === current));
  if (!listed) {
    const a = config.audio;
    const text = a.app
      ? `${a.app_mode === 'except' ? 'Everything except ' : ''}${appLabel(a.app)} · not running`
      : `${current.replace(/^(out|in):/, '')} · not connected`;
    groups.unshift(opt(current, text));
  }
  r.select.replaceChildren(...groups);
  r.select.value = current;
}

async function refreshSources() {
  const sources = await api('/api/sources');
  devices = sources.devices || [];
  defaultDevice = sources.default || '';
  apps = sources.apps || [];
  renderSourceOptions();
}

function renderAudioInput() {
  const select = h('select', { 'aria-label': 'Listen to' });
  select.addEventListener('change', () => setSource(select.value));
  const status = h('p', { class: 'ai-status', role: 'status' }, [h('i'), h('span')]);
  const meters = ['L', 'R'].map((name) => {
    const cover = h('i', { class: 'ai-cover' });
    const value = h('output', { class: 'mono' });
    const row = h('div', { class: 'ai-row' }, [h('span', { class: 'ai-ch' }, name), h('div', { class: 'ai-meter' }, cover), value]);
    return { row, cover, value, last: '' };
  });
  const clip = h('span', { class: 'ai-clip', title: 'Lights up when the audio reaches full scale (0 dBFS) and distorts' }, 'CLIP');
  const gain = h('input', { type: 'range', min: -24, max: 24, step: 0.5, 'aria-label': 'Input gain', title: 'Double-click to reset' });
  const gainOut = h('output', { class: 'mono' });
  const setGain = (db) => {
    config.audio.gain = db;
    syncAudioInput();
    commit();
  };
  gain.addEventListener('input', () => setGain(Number(gain.value)));
  gain.addEventListener('dblclick', () => setGain(0));
  const auto = h('input', { type: 'checkbox', role: 'switch', 'aria-label': 'Auto level' });
  auto.addEventListener('change', () => {
    config.audio.auto_level = auto.checked;
    syncAudioInput();
    commit();
  });
  const autoOut = h('output', { class: 'mono ai-auto', title: 'Gain auto level adds right now' });

  $('#audio-input-body').replaceChildren(
    h('div', { class: 'field' }, [h('label', null, 'Listen to'), h('div', { class: 'ctl' }, select)]),
    status,
    h('div', { class: 'ai-meters' }, [h('div', { class: 'ai-rows' }, meters.map((m) => m.row)), clip]),
    h('div', { class: 'field' }, [h('label', null, 'Gain'), h('div', { class: 'ctl' }, [gain, gainOut])]),
    h('div', { class: 'field' }, [h('label', null, 'Auto level'), h('div', { class: 'ctl' }, [
      h('label', { class: 'switch' }, [auto, h('span', { class: 'track' })]), autoOut,
    ])]),
    h('p', { class: 'note' }, 'Only one app: the lights follow just that app, so game sounds and notifications leave them alone. '
      + 'Auto level keeps quiet and loud songs at a similar strength, so level triggers work at any volume.'),
  );
  audioRefs = { select, status, meters, clip, gain, gainOut, auto, autoOut, lastStatus: '' };
  renderSourceOptions();
  syncAudioInput();
  // Keep the app list (and who is playing) current while the card is open
  setInterval(() => {
    if (document.hidden || $('#audio-input').classList.contains('collapsed')) return;
    refreshSources().catch(() => {});
  }, SOURCE_REFRESH_MS);
}

const fmtDb = (db) => `${db > 0 ? '+' : ''}${db.toFixed(1)} dB`;

function syncAudioInput() {
  const r = audioRefs;
  if (!r) return;
  if (document.activeElement !== r.select) {
    r.select.value = sourceValue();
    if (r.select.value !== sourceValue()) renderSourceOptions();
  }
  if (document.activeElement !== r.gain) r.gain.value = config.audio.gain;
  r.gainOut.textContent = fmtDb(config.audio.gain);
  r.auto.checked = config.audio.auto_level;
}

// The live part, every frame: status line, meters (transforms only) and the auto gain
function updateAudioInput() {
  const r = audioRefs;
  if (!r) return;
  const input = online && live.status && live.status.input;
  let state = 'err';
  let text = 'The studio is offline';
  if (input) {
    if (input.state === 'ok') {
      const silent = input.packets === 0 && input.kind === 'output';
      state = silent ? 'idle' : 'ok';
      const parts = [`${input.rate / 1000} kHz`, `${input.channels} ch`];
      if (silent) parts.unshift('Nothing playing');
      else parts.push(`${input.packets} packets/s`, `${input.delay} ms delay`);
      if (input.glitches) parts.push(`${input.glitches} glitch${input.glitches === 1 ? '' : 'es'}`);
      text = parts.join(' · ');
    } else {
      state = input.state === 'error' ? 'err' : 'idle';
      text = input.message || 'Starting…';
    }
  }
  const key = `${state}|${text}`;
  if (key !== r.lastStatus) {
    r.lastStatus = key;
    r.status.className = `ai-status ${state}`;
    r.status.lastChild.textContent = text;
    r.status.title = input && input.event
      ? 'Event driven: each packet is analysed the moment Windows delivers it. Delay: from Windows stamping the audio to the studio analysing it.'
      : '';
  }
  const peaks = input ? input.peak : [];
  r.meters.forEach((m, i) => {
    const db = peaks[i] ?? -90;
    const fraction = clamp((db - METER_MIN_DB) / -METER_MIN_DB, 0, 1);
    setMeter(m.cover, `scaleX(${(1 - fraction).toFixed(3)})`);
    const label = db <= METER_MIN_DB ? '–' : db.toFixed(0);
    if (label !== m.last) {
      m.last = label;
      m.value.textContent = label;
    }
  });
  r.clip.classList.toggle('on', !!(input && input.clip));
  const autoText = input && config.audio.auto_level ? fmtDb(input.auto) : '';
  if (r.autoOut.textContent !== autoText) r.autoOut.textContent = autoText;
}

/* ---------- live data ---------- */

// The config changed outside this page: another tab, the server following a
// new Hyperion led count (it scales the segments), or a server restart.
// Only reloads while this page has no edits of its own on the way.
let reloading = false;

function configIsStale() {
  if (live.server_id === undefined) return live.led_count && live.led_count !== config.led_count;
  return live.server_id !== config.server_id || live.revision > config.revision;
}

async function reloadFromServer() {
  if (reloading) return;
  reloading = true;
  const before = config.led_count;
  try {
    applyState(await api('/api/state'));
    toast(config.led_count !== before
      ? `Hyperion now has ${config.led_count} LEDs (was ${before}). Segments were scaled to fit.`
      : 'Synced with the latest settings from the studio.');
  } catch (err) {
    toast(`Could not load the latest settings: ${err.message}`);
  } finally {
    reloading = false;
  }
}

function connectStream() {
  const stream = new EventSource('/api/stream');
  stream.onopen = () => { online = true; };
  stream.onmessage = (e) => {
    live = JSON.parse(e.data);
    ingestSpectrum(live.spectrum);
    ledBytes = hexToBytes(live.leds || '');
    online = true;
    invalidate();
    if (!dragging && !pendingSave && !saving && configIsStale()) reloadFromServer();
  };
  stream.onerror = () => {
    online = false;
    invalidate();
  };
}

function frame() {
  frameRequested = false;
  if (config) {
    // Canvases in a collapsed card have no size and are skipped
    if (anCanvas.clientWidth) drawAnalyzer();
    if (stripCanvas.clientWidth) drawStrip();
    if (trackCanvas.clientWidth) {
      drawTrack();
      syncTrackBar();
    }
    updateMeters();
    updateStatus();
  }
}

async function init() {
  try {
    applyState(await api('/api/state'));
  } catch (err) {
    toast(`Could not load the studio: ${err.message}`);
    setTimeout(init, 2000);
    return;
  }
  setupAnalyzer();
  setupStrip();
  setupControls();
  renderAmbient();
  renderAudioInput();
  setupReactive();
  setupColorRanges();
  setupCollapsibles();
  renderPresets();
  renderColorRanges();
  // Anything the user does can change what the canvases show, and so can a
  // canvas changing size (window resize, a card collapsing or expanding)
  for (const type of ['pointerdown', 'pointermove', 'pointerup', 'pointercancel', 'keydown', 'click', 'input', 'change', 'wheel']) {
    document.addEventListener(type, invalidate, { capture: true, passive: true });
  }
  const resized = new ResizeObserver(invalidate);
  for (const canvas of [anCanvas, stripCanvas, trackCanvas]) resized.observe(canvas);
  connectStream();
  invalidate();
}

init();
