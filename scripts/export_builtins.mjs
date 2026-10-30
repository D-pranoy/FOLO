#!/usr/bin/env node
// Exports the studio's built-in looks (genre presets, effects and LedFx
// effects) as plain segment lists, by evaluating the builder functions of
// web/studio.js in Node with stubs in place of the DOM (RB-01 step 7).
//
//   node scripts/export_builtins.mjs [--led-counts 30,61,144] [--out FILE]
//   node scripts/export_builtins.mjs --trace [--out FILE]
//
// Without --out the JSON goes to stdout. Segment ids are numbered instead
// of random, so the same studio.js always gives the same output. The golden
// frames (scripts/make_golden.py) and the content library (RB-09) read it.
//
// --trace records what the built-in packs are made of instead (RB-09,
// ADR-0038 D18): for each built-in and segment, the fractions the builder
// used and the segment, plus the palettes, reactive presets, categories and
// descriptions. It checks, at every length of TRACE_LENGTHS, that the
// fractions give the builder's cells under `snap: nearest` and that nothing
// else of a segment depends on the length.

import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const DEFAULT_LED_COUNTS = [30, 61, 144];
// The lengths the fractions are checked at: ADR-0038 D20's equivalence lengths
const TRACE_LENGTHS = [
  ...Array.from({ length: 64 }, (_, i) => i + 1), 100, 144, 255, 256, 300, 512, 1000, 1024, 1999, 2000,
];

function parseArgs(argv) {
  const args = { ledCounts: DEFAULT_LED_COUNTS, out: null, trace: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--led-counts') {
      args.ledCounts = argv[++i].split(',').map((s) => {
        const n = Number(s);
        if (!Number.isInteger(n) || n < 1) throw new Error(`Bad LED count: ${s}`);
        return n;
      });
    } else if (argv[i] === '--trace') {
      args.trace = true;
    } else if (argv[i] === '--out') {
      args.out = argv[++i];
    } else {
      throw new Error(`Unknown argument: ${argv[i]}`);
    }
  }
  return args;
}

// Stands in for any DOM object: every property is another stub, calling or
// constructing one returns a stub, and it reads as 0 or '' where a value is needed.
function stub() {
  const target = function () {};
  return new Proxy(target, {
    get(_, key) {
      if (key === Symbol.toPrimitive) return (hint) => (hint === 'number' ? 0 : '');
      if (key === Symbol.iterator) return function* () {};
      if (key === 'then') return undefined; // not a promise
      return stub();
    },
    set: () => true,
    apply: () => stub(),
    construct: () => stub(),
    has: () => true,
  });
}

// Ids from crypto.randomUUID, numbered from 1 after every reset
let idCounter = 0;
function nextUuid() {
  idCounter += 1;
  return `${idCounter.toString(16).padStart(8, '0')}-0000-4000-8000-000000000000`;
}

// Every segment comes from part() or rangeSegment() (inside layoutRanges, after
// its span()); wrap them to remember the fractions each used, by segment.
function traceFractions(source) {
  const patches = [
    ['function part(n, name, from, to, react, palette, extra = {}) {',
      'function __part(n, name, from, to, react, palette, extra = {}) {'],
    ['function rangeSegment(name, start, end, range) {', 'function __rangeSegment(name, start, end, range) {'],
    ['  const span = (from, to) => {\n', '  const span = (from, to) => {\n    globalThis.__spanLast = { from, to };\n'],
  ];
  let code = source;
  for (const [from, to] of patches) {
    if (!code.includes(from)) throw new Error(`web/studio.js changed: update traceFractions() in scripts/export_builtins.mjs (${from})`);
    code = code.replace(from, to);
  }
  // The script ends with init(): the wrappers go before it
  return code.replace(/\ninit\(\);\s*$/, `
const __fractions = new WeakMap();
function part(n, name, from, to, ...rest) {
  const seg = __part(n, name, from, to, ...rest);
  __fractions.set(seg, { from, to });
  return seg;
}
function rangeSegment(...args) {
  const seg = __rangeSegment(...args);
  __fractions.set(seg, globalThis.__spanLast || { from: 0, to: 1 });
  globalThis.__spanLast = null;
  return seg;
}
init();
`);
}

function loadStudio(path) {
  const source = readFileSync(path, 'utf8');
  // The script ends by starting the page; export the builders instead.
  const start = /\ninit\(\);\s*$/;
  if (!start.test(source)) {
    throw new Error(`${path} no longer ends with "init();": update scripts/export_builtins.mjs`);
  }
  const code = traceFractions(source).replace(start,
    '\nglobalThis.__builtins = { GENRE_PRESETS, EFFECTS, LEDFX_EFFECTS, layoutRanges, effectId,'
    + ' PRESETS, REACTIVE, CATEGORIES, fractions: __fractions };\n');
  const context = {
    console,
    document: stub(),
    localStorage: stub(),
    navigator: stub(),
    location: stub(),
    crypto: { randomUUID: nextUuid },
    fetch: () => new Promise(() => {}),
    setTimeout: () => 0,
    clearTimeout: () => {},
    setInterval: () => 0,
    clearInterval: () => {},
    requestAnimationFrame: () => 0,
    cancelAnimationFrame: () => {},
    matchMedia: () => stub(),
    getComputedStyle: () => stub(),
    ResizeObserver: class { observe() {} disconnect() {} },
    EventSource: class { close() {} },
    Image: class {},
    OffscreenCanvas: class {},
  };
  context.window = context;
  context.globalThis = context;
  vm.createContext(context);
  vm.runInContext(code, context, { filename: path });
  return { builtins: context.__builtins, sha256: createHash('sha256').update(source).digest('hex') };
}

// The cells `snap: nearest` gives a fraction pair (docs/architecture/pipeline.md)
const roundHalfUp = (x) => (x - Math.floor(x) >= 0.5 ? Math.floor(x) + 1 : Math.floor(x));
function nearestCells(from, to, n) {
  const start = Math.min(roundHalfUp(from * n), n - 1);
  return [start, Math.min(Math.max(start, roundHalfUp(to * n) - 1), n - 1)];
}

const POSITION_KEYS = new Set(['id', 'start', 'end']);
const withoutPosition = (seg) => Object.fromEntries(Object.entries(seg).filter(([k]) => !POSITION_KEYS.has(k)));

// A built-in as layers: the segments' fields and fractions, taken from the
// first length and checked at all of them.
function traceLook(look, fractions) {
  const reference = look.build(TRACE_LENGTHS[0]);
  const layers = reference.map((seg) => {
    const fraction = fractions.get(seg);
    if (!fraction) throw new Error(`${look.id}: segment ${seg.name} wasn't made by part() or rangeSegment()`);
    return { from: fraction.from, to: fraction.to, ...withoutPosition(seg) };
  });
  for (const n of TRACE_LENGTHS) {
    const segments = look.build(n);
    if (segments.length !== layers.length) throw new Error(`${look.id}: ${segments.length} segments at ${n} LEDs`);
    segments.forEach((seg, i) => {
      const fraction = fractions.get(seg);
      const [start, end] = nearestCells(fraction.from, fraction.to, n);
      // The builders don't keep to the strip on a very short one; the page's
      // sanitising does (tests/test_builtin_packs.py checks it with the real one)
      const first = Math.min(seg.start, n - 1);
      const last = Math.min(Math.max(seg.end, first), n - 1);
      if (first !== start || last !== end) {
        throw new Error(`${look.id} ${seg.name} at ${n}: the builder gives ${first}-${last}, snap nearest ${start}-${end}`);
      }
      const { from, to, ...fields } = { ...layers[i] };
      const expected = JSON.stringify({ ...fields, from, to });
      const actual = JSON.stringify({ ...withoutPosition(seg), from: fraction.from, to: fraction.to });
      if (expected !== actual) throw new Error(`${look.id} ${seg.name}: fields change with the length (${n})`);
    });
  }
  return JSON.parse(JSON.stringify(layers));
}

function trace(builtins, sha256) {
  const { GENRE_PRESETS, EFFECTS, LEDFX_EFFECTS, layoutRanges, effectId, PRESETS, REACTIVE, CATEGORIES, fractions } = builtins;
  const looks = [
    ...GENRE_PRESETS.map((g) => ({ ...g, kind: 'genre', build: (n) => layoutRanges(n, g.layout, g.ranges) })),
    ...EFFECTS.map((e) => ({ ...e, id: effectId(e), kind: 'effect' })),
    ...LEDFX_EFFECTS.map((e) => ({ ...e, id: effectId(e), kind: 'ledfx' })),
  ];
  return {
    source: 'web/studio.js',
    source_sha256: sha256,
    lengths: TRACE_LENGTHS,
    palettes: Object.entries(PRESETS).map(([name, colors]) => ({ name, colors })),
    reactions: REACTIVE.flatMap(([group, presets]) => presets.map((p) => ({
      group, name: p.name, tempo: p.tempo, anim: p.anim, dur: p.dur, desc: p.desc, range: p.range, response: p.response,
    }))),
    categories: CATEGORIES.map((c) => ({ id: c.id, name: c.name, blurb: c.blurb, items: c.items || [] })),
    builtins: looks.map((look) => {
      idCounter = 0;
      return {
        id: look.id, kind: look.kind, name: look.name, desc: look.desc, tempo: look.tempo,
        tags: look.tags || [], source: look.source || '',
        layers: traceLook(look, fractions),
      };
    }),
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const path = fileURLToPath(new URL('../web/studio.js', import.meta.url));
  const { builtins, sha256 } = loadStudio(path);
  if (args.trace) {
    const text = `${JSON.stringify(trace(builtins, sha256), null, 1)}\n`;
    if (args.out) writeFileSync(args.out, text);
    else process.stdout.write(text);
    return;
  }
  const { GENRE_PRESETS, EFFECTS, LEDFX_EFFECTS, layoutRanges, effectId } = builtins;

  const looks = [
    ...GENRE_PRESETS.map((g) => ({
      id: g.id, kind: 'genre', name: g.name, tags: g.tags,
      build: (n) => layoutRanges(n, g.layout, g.ranges),
    })),
    ...EFFECTS.map((e) => ({ id: effectId(e), kind: 'effect', name: e.name, tags: e.tags, build: e.build })),
    ...LEDFX_EFFECTS.map((e) => ({ id: effectId(e), kind: 'ledfx', name: e.name, tags: e.tags, build: e.build })),
  ];
  const ids = new Set();
  for (const look of looks) {
    if (ids.has(look.id)) throw new Error(`Duplicate built-in id: ${look.id}`);
    ids.add(look.id);
  }

  const output = {
    source: 'web/studio.js',
    source_sha256: sha256,
    led_counts: args.ledCounts,
    builtins: looks.map((look) => ({
      id: look.id,
      kind: look.kind,
      name: look.name,
      tags: look.tags || [],
      segments: Object.fromEntries(args.ledCounts.map((n) => {
        idCounter = 0;
        // Round trip through JSON: plain data, as the page posts it
        return [String(n), JSON.parse(JSON.stringify(look.build(n)))];
      })),
    })),
  };
  const text = `${JSON.stringify(output, null, 1)}\n`;
  if (args.out) writeFileSync(args.out, text);
  else process.stdout.write(text);
}

main();
