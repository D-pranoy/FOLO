# The current system (before the rebuild)

Status: Accepted (describes the code as it exists at the start of the rebuild)

This is the baseline that the rebuild replaces. Its behaviour is preserved where the [UI parity checklist](../specs/ui-parity-checklist.md) and the [parity mode](pipeline.md#parity-mode) require it. `AGENTS.md` remains the guide to running it until it is replaced.

## Shape

| Part | Files | Notes |
|---|---|---|
| Studio server | `studio.py` | stdlib `ThreadingHTTPServer`, bound to `127.0.0.1:8777`. Refuses non-local `Host` headers and non-JSON POSTs. Serves `web/`, `GET /api/state`, `GET /api/stream` (server-sent events at 30 fps), `POST /api/config` (whole config), `POST /api/reset`, `POST /api/layout`. Polls Hyperion's LED count every 5 s. |
| Config | `app/studio_config.py` | `sanitize_config()` clamps every value and never rejects. `ConfigStore` replaces the config dict wholesale, debounces saves to `studio.json`, and keeps `revision` and `server_id`. |
| Engine | `app/studio_engine.py` | Capture thread, `Analyzer`, `Renderer` and `HyperionSender` (described below) |
| JSON client | `app/json_client.py` | Hyperion JSON `color` commands with `duration` |
| UI | `web/studio.js` (2,965 lines), `index.html`, `studio.css` | Vanilla JS, no framework or build step; works offline |
| Legacy effects | `main.py`, `effects/`, `lib/`, `app/gui.py`, `app/hyperion.py`, `matrix-config.js`, `toggle.sh`, `.asoundrc`, `config.json` | The upstream Python 2 effect runner, ported to Python 3. Not used by the studio. |

## Engine details worth keeping

- **Capture:** soundcard WASAPI loopback at 48 kHz.
  - `record(numframes=None)` takes each 10 ms packet as WASAPI delivers it.
  - Between packets the loop sleeps for 70% of the device period.
- **Analysis:**
  - Hann-windowed FFTs: 4096 points (`full`: level triggers and the display) and 2048 points (`fast`: hits triggers, computed only when needed).
  - Band level is the loudest bin in the range, interpolated for bands narrower than a bin (`np.maximum.reduceat` with cached plans).
  - The power of every channel that has carried sound (above −100 dBFS, sticky) is averaged.
  - `CALIBRATION_DB = −9.54 dB` keeps the scale the presets were tuned on.
- **Rendering per segment:**
  1. Band level, plus a 1 s running average for hits.
  2. Shaping: threshold → 0, ceiling → 1, then gain, then the curve exponent.
  3. Attack/release smoothing.
  4. The mode, laid out by direction with step indices; mirrored directions get half the steps.
  5. A max-blend into the frame, multiplied by `master`.
- **Moving modes** (scroll, drops, scan, wipe, flow, fire) keep per-segment state. Hits fire when the shaped value rises past 0.5 after falling below 0.25, at most one per 60 ms.
- **Hyperion output:** priority 100, origin `Audio Effects`, `duration` 2000 ms, so Hyperion releases the LEDs if the studio dies.

## Built-in content

The menu holds 44 built-ins:
- 12 effects and 18 LedFx-inspired effects, built by JavaScript builder functions
- 14 genre presets, which use layout data and refer to reactive presets by name

There are also 20 reactive presets in 4 groups, 9 palettes, 6 colour ranges and 8 menu categories.

## UI

- **Design tokens:**
  - `--bg #08080d`, `--accent #b36bff`, `--accent-2 #ff4fa8`, `--radius 12px`, the system-ui font stack
  - Deep Night palette: `#0033CC`, `#000080`, `#4B00FF`, `#FF3399`, `#FF00FF`, `#800080`, `#8F00FF`, `#4B0082`
- **Layout:** three columns (290 px | flexible | 380 px), two at ≤1500 px, one at ≤1100 px, a compact phone layout at ≤640 px.
- The full feature inventory is the [UI parity checklist](../specs/ui-parity-checklist.md).

## Known defects (to fix in the rebuild)

**Server**
- An unguarded `int(Content-Length)` (`studio.py:130`).
- `revision += 1` outside the lock (`studio_config.py:327`).
- File I/O without an explicit encoding.
- The capture thread is never joined.
- No SIGTERM handling; clean shutdown on Ctrl+C only.
- The renderer's RNG has no seed, so outputs can't be reproduced in tests.

**UI**
- An Undo button stays clickable after its toast disappears.
- The preset edit form keeps a stale reference and can lose changes.
- The brightness readout isn't clamped.
- The inspector's attack/release limits (1000/3000 ms) are below the server's (2000/5000 ms), yet built-ins use 1200/4000.
- A stale "Presets" label.
- Inconsistent destructive actions: some have Undo, Clear and Auto split don't.
- No keyboard path for canvas editing.

## Measured performance

See [pipeline](pipeline.md#performance-budget).

## Licensing note

The repository is MIT (upstream © Juha Rantanen). The six moving modes and 18 effects were ported with reference to LedFx's source, and LedFx is GPL-3.0. They are rewritten clean-room before the first public binary release ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)).
