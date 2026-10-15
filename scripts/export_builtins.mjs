#!/usr/bin/env node
// Exports the studio's built-in looks (genre presets, effects and LedFx
// effects) as plain segment lists, by evaluating the builder functions of
// web/studio.js in Node with stubs in place of the DOM (RB-01 step 7).
//
//   node scripts/export_builtins.mjs [--led-counts 30,61,144] [--out FILE]
//
// Without --out the JSON goes to stdout. Segment ids are numbered instead
// of random, so the same studio.js always gives the same output. The golden
// frames (scripts/make_golden.py) and the content library (RB-09) read it.

import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const DEFAULT_LED_COUNTS = [30, 61, 144];

function parseArgs(argv) {
  const args = { ledCounts: DEFAULT_LED_COUNTS, out: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--led-counts') {
      args.ledCounts = argv[++i].split(',').map((s) => {
        const n = Number(s);
        if (!Number.isInteger(n) || n < 1) throw new Error(`Bad LED count: ${s}`);
        return n;
      });
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

function loadStudio(path) {
  const source = readFileSync(path, 'utf8');
  // The script ends by starting the page; export the builders instead.
  const start = /\ninit\(\);\s*$/;
  if (!start.test(source)) {
    throw new Error(`${path} no longer ends with "init();": update scripts/export_builtins.mjs`);
  }
  const code = source.replace(start,
    '\nglobalThis.__builtins = { GENRE_PRESETS, EFFECTS, LEDFX_EFFECTS, layoutRanges, effectId };\n');
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

function main() {
  const args = parseArgs(process.argv.slice(2));
  const path = fileURLToPath(new URL('../web/studio.js', import.meta.url));
  const { builtins, sha256 } = loadStudio(path);
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
