# AGENTS.md

Guidance for AI coding agents (Claude Code, Codex and others) working in this repository.

## What this is

**FOLO** (formerly Vibearion): a Windows / Python 3 fork of RanzQ/hyperion-audio-effects (2017, Python 2, Linux GStreamer). The main product is the **FOLO studio** ("Fear of lights out", formerly the Audio Studio): a local web UI where LED segments are mapped to frequency ranges and rendered live to [Hyperion.NG](https://github.com/hyperion-project/hyperion.ng), which drives a WLED strip. The original effect scripts (`main.py` + `effects/`) still work, ported to Python 3.

No build step, no test suite, no installed linter (`.pylintrc` exists for style; pylint isn't in the venv).

## Commands

Everything runs from the project's venv (Python 3.13). Recreate it with:

```
py -3.13 -m venv .venv
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

| Task | Command |
|---|---|
| Run the studio (opens http://127.0.0.1:8777/) | `studio.bat` or `.venv\Scripts\python.exe studio.py [--port 8777] [--no-browser] [--config studio.json]` |
| Run a legacy effect | `run.bat --effect color_spectrum` / `--effect vumeter` (type `x` + Enter to stop) |
| List audio capture devices | `run.bat --list-devices` |
| Syntax check Python | `.venv\Scripts\python.exe -m py_compile studio.py app\studio_config.py app\studio_engine.py app\json_client.py main.py` |
| Syntax check the UI | `node --check web\studio.js` |
| Ask Hyperion for its LED layout | JSON API: send `{"command":"serverinfo","tan":1}\n` to `localhost:19444` (see `app.json_client.fetch_server_info`) |

**Restart rules:** Python changes need the studio process restarted (kill the `python.exe` running `studio.py`, then start `studio.bat` again; LEDs blink briefly). `web/*` files are served with `no-store`, so UI changes only need a page refresh.

**Testing without touching the live setup:** `studio.json` is the live config, edited through the UI, and the studio drives real LEDs. Test destructive or experimental changes on a second instance with a copied config and Hyperion output disabled:

```
# copy studio.json elsewhere, set "hyperion": {"enabled": false, ...} in the copy, then
.venv\Scripts\python.exe studio.py --port 8778 --no-browser --config <copy>.json
```

Engine logic can be tested offline by importing `app.studio_engine.Analyzer` / `Renderer` and feeding numpy blocks, frames × channels or mono (see how `Renderer.render(config, spectrum, dt)` is called in `StudioEngine._process`). For A/B work on real audio, record the capture device to a `.npy` (`device.recorder(samplerate=48000).record(numframes=...)`) and feed both engines the same 480-frame packets so their frames line up. Stacking the frames of one segment into an image (time down, LEDs across) is a quick way to see whether a moving mode looks right. Browsers throttle timers in hidden tabs to about 1 s, so in-page JS checks need multi-second waits.

`.claude/launch.json` (git-ignored) has a `studio-test` configuration for the in-app browser preview: port 8778 with a config copy in a session scratchpad. Point `--config` at your own copy before using it.

## Architecture

### Studio server (`studio.py`)
`ThreadingHTTPServer` bound to 127.0.0.1. It refuses requests whose `Host` isn't local, and POSTs that aren't `application/json`.
- `GET /` serves `web/`
- `GET /api/state`: config, audio devices, display constants
- `GET /api/stream`: server-sent events at 30 fps
- `POST /api/config`: the page always posts the **whole** config
- `POST /api/reset`
- `POST /api/layout`: re-read the LED count from Hyperion

A watcher thread polls Hyperion's LED count every 5 s and calls `ConfigStore.set_led_count`, which scales all segments proportionally.

### Config (`app/studio_config.py`)
`sanitize_config()` is the single source of truth for the schema and its limits. Everything the UI sends goes through it.

`ConfigStore` behaviour:
- **Replacement:** the config dict is replaced as a whole on every change, never mutated, so readers use it without locks.
- **Saving:** writes to `studio.json` (git-ignored, per-machine state) are debounced.
- **Version tags:** `revision` and `server_id` let open pages detect outside changes and reload.
- **Stale posts:** a POST carrying an old `led_count` gets its segments scaled to the current count.
- **Reset:** `reset()` keeps `audio`, `hyperion`, `presets` and `default_preset`.

Segment fields:
- `start`, `end`: LED indices
- `freq_lo`, `freq_hi`: Hz
- `trigger`: `level` or `hits`
- `threshold`, `ceiling`
- `gain`, `curve`, `attack`, `release` (ms), `floor` (idle glow, 0–1)
- `mode`: `fill`, `pulse`, `spectrum` or `shift`, or one of the moving modes ported from LedFx: `scroll`, `drops`, `scan`, `wipe`, `flow`, `fire`
- `direction`: `forward`, `reverse`, `center` or `edges`
- `speed` (LEDs per second, 0–2000) and `width` (% of the segment, 1–100): only used by the moving modes
- `peak`, `palette` (hex list), `enabled`, `solo`
- `lane`: the segment's row on the UI's LED track (0–15), or −1 to be placed automatically. The engine ignores it.

**Threshold units depend on `trigger`:**
- **`level`:** absolute dBFS, threshold −120…−1 and ceiling −119…0.
- **`hits`:** dB *above the band's 1 s running average*, threshold 0…39 and ceiling 1…40.

The JS `enforce()` / `dbLimits()` must stay in sync with these limits.

### Engine (`app/studio_engine.py`)
- **Capture thread:** a `soundcard` WASAPI loopback (48 kHz). `record(numframes=None)` takes each packet as WASAPI delivers it (480 frames every 10 ms), so there are about 100 analysis frames per second, always on the newest audio and never on a backlog. Between packets the loop sleeps for 70% of the device period instead of letting soundcard poll every 0.75 ms (0.3% of a core instead of 1.6–3%). `dt` is the packet length, capped at `MAX_DT`.
- **`Analyzer`:** Hann-windowed FFTs of the latest samples, returned as a `Spectrum`. A band's level is the loudest bin in its range, interpolated when the band is narrower than one bin (`band_levels()`, vectorised with `np.maximum.reduceat` and cached index plans).
  - **Two windows:** `full` uses 4096 points (11.7 Hz bins, 85 ms). It drives the UI's spectrum and level triggers. `fast` uses 2048 points and drives hits triggers, including spectrum mode on hits. Measured on real music, hits light 10–20 ms sooner (a mixed-in snare burst: 16.7 → 10 ms), with the same share of time lit. `fast` is only computed when a hits segment asks for it.
  - **What hits report:** hits segments report the `full` level and its own running average (`shown_average`) in `levels`, so the UI's lines still sit on the spectrum it draws.
  - **Channels:** the power of every channel that has carried sound (louder than −100 dBFS, sticky) is averaged. The capture device is 5.1, but only FL/FR carry audio, and the other four hiss at about −190 dB rather than being digitally silent. The old code averaged all six channels into mono, which cancelled out-of-phase stereo content and read 9.5 dB low.
  - **dB scale:** `CALIBRATION_DB` (−9.54 dB) keeps the scale the presets were tuned on, whatever the device's channel layout. A full scale 1 kHz sine on L+R reads −10.2 dB before and after. Compared frame by frame, readings are never lower than the old engine's: typically +1 dB, and more where stereo content used to cancel.
- **`Renderer`, per segment:**
  1. Band level, plus a running average (`AVERAGE_MS`) used by hits.
  2. Shaping: threshold maps to 0, ceiling to 1, then gain, then the `curve` exponent.
  3. Attack/release smoothing.
  4. The mode, laid out by direction, using a step-index layout where mirrored directions get half the steps.
  5. A max-blend into the frame, then multiplication by `master`.
- **Moving modes** (`MOTION_MODES`, after LedFx effects) keep per-segment state in `SegmentState` and draw RGB per step themselves:
  - `scroll`: the color of the moment (palette by level) streams away at `speed`; the trail fades out over `width`.
  - `drops`: each hit spawns a ripple at a random step that grows at `speed` (0 = still spot), `width` wide, fading with `release`.
  - `scan`: an eye `width` long bounces between the ends at `speed` × level.
  - `wipe`: each hit sweeps the next palette color across at `speed`, eased out and turning around each time; under 100% `width` a bar travels instead.
  - `flow`: sine-wave palette blobs `width` long drift at `speed`, pushed faster by the level.
  - `fire`: base sparks heat the segment, heat rises at `speed`, and flames reach higher with the level.
  - **Hits** for drops and wipe: the shaped target rising past `HIT_ON` (0.5) after dropping below `HIT_OFF` (0.25), at most one per `HIT_GAP`.
  - **Idle glow** (`floor`) is the minimum brightness of what the mode draws (scan eye, wipe colors, flow, flame height); scroll and drops use it as a dim palette backdrop.
- **`HyperionSender` thread:** sends JSON `color` commands at the configured fps with priority 100, origin `Audio Effects` and `duration` 2000 ms, so Hyperion releases the LEDs if the studio dies. Hyperion rejects an `origin` longer than 20 characters.
- **Stream snapshot fields:** `spectrum` (192 log-spaced bands, 20 Hz–20 kHz, as dB integers; computed once per analysed packet and shared by every open page), `leds` (hex), `levels {id: [raw dB, value 0–1, average dB]}`, `led_count`, `revision`, `server_id` and `status`.

### Web UI (`web/studio.js`, `index.html`, `studio.css`)
A single-file vanilla JS app with no framework or bundler, and no CDN, so it works offline.

**Edit flow:**
1. The page keeps the full `config`, and every edit mutates it then calls `commit()`.
2. `commit()` is a debounced POST of the whole config.
3. The server's sanitized response replaces `config` only if no newer edits happened in the meantime (`editSeq`, `pendingSave`, `saving`).
4. The stream triggers `reloadFromServer()` when `revision` or `server_id` moved and this page has no pending edits.

**Drawing:** canvases are redrawn every `requestAnimationFrame` from the latest stream data.
- **Analyzer:** segment boxes, with hits boxes positioned at the live average plus their offset.
- **LED strip and lane track:** drag to create, move or resize segments. The bar above the track has + Lows, + Mids, + Highs and Delete (`addKind()`, `deleteSelected()`). A new segment copies the settings of the last segment of its kind (`kindOf()`: by name, else by the band's center; `kindDefaults()` when there is none), takes the lowest free number (Mids 2 after Mids 2 was deleted), and goes into the longest free run of LEDs, or else the right half of the selected (or widest) segment. It is listed in strip order. Add and delete both toast an Undo (`undoToast()`).
  - **Rows:** `computeLanes(last)` places segments with a `lane` in that row and the rest (−1) in the first row where they fit. A segment that would overlap another in its row moves down (`last` is the one to move), and empty rows close up. Dragging a segment up or down moves it to another row, or to a new one via the empty row at the bottom. Dragging on empty space creates a segment on that row. After every drag `pinLanes()` / `settleLanes()` store the displayed row of every segment. Overlapping segments in different rows still max-blend on the LEDs.

**Layout:** three grid areas: the effects menu on the left (`aside#presets`, sticky below the top bar via the `--topbar-h` variable), the editor (`.stage`) in the middle, and the inspector (`.side`) on the right. Under 1500 px the inspector moves below the editor; under 1100 px everything stacks.

**Panels:** the effects menu, segment list, inspector (built from `rangeField` / `segmented` helpers that register `syncers`), Colors by range, Reactive presets and Settings. The inspector shows Speed and Size only for moving modes, and names Size after the mode (`SIZE_LABEL`).

**Collapsing:** every card with `data-collapse="<key>"` gets an arrow button in its `h2` (`setupCollapsibles()`). A collapsed card (`.collapsed`) shows only its title, and the state is remembered in `localStorage` (`studio.collapsedCards`). `data-collapsed` makes a card start collapsed (Settings). The inspector card wraps `#inspector`, which is rebuilt on every selection, under a static "Selected segment" head. Canvases in a collapsed card have no size, so `frame()` skips drawing them.

**Segment defaults:** every segment builder (`part`, `rangeSegment`, `addSegment`, `autoSplit`) spreads `MOTION_DEFAULTS` (speed 120, width 20, the server's defaults). `EFFECT_KEYS` compares speed and width when detecting the active effect, so a built-in without them would never show as active.

**Brightness:** the sliders are perceptual (position^2.2). `config.master` stores the linear output scale.

**Presets library** (`library()`):
- **Saved presets:** from `config.presets`; they store `led_count` and are scaled on apply.
- **Genre presets:** from `GENRE_PRESETS`, built by `layoutRanges()`. Layouts are alternate, zones, mirror or stack. Each range names a per-segment preset from `REACTIVE`; that preset's `range` supplies trigger and thresholds, and its `response` supplies motion.
- **Effects:** custom `EFFECTS` builds from `REACT` building blocks.
- **LedFx effects:** `LEDFX_EFFECTS`, 18 ports of LedFx's audio reactive effects (from its source, `ledfx/effects` on GitHub, since its docs only cover a few). They use LedFx's lows (30–250 Hz), mids (250–3000 Hz) and highs (3–10 kHz) as `LFX_LOWS` / `LFX_MIDS` / `LFX_HIGHS`, in Deep Night colors. Each has `source: 'LedFx'` (a badge in the menu), a `LedFx` tag and an id `fx:ledfx-<name>` (`effectId()`). LedFx's BPM effects (Bar, Multicolor bar) use hits because the studio has no beat tracker.

**Effects menu:** the library is grouped under umbrella categories in `CATEGORIES`: My presets, LedFx, Electronic, Hip-hop & R&B, Pop & Rock, Jazz & Classical, Chill & Ambient and Visualizers.
- **Membership:** built-ins are listed in categories by id, saved presets always go to My presets, and anything left unlisted falls into a "More" group. Add a new built-in to a category's `items`.
- **Search:** matches name, description, tags, category and range names and reactions.
- **Interaction:** a click applies the effect, and ▾ expands its details (ranges, tags, ★ default, Edit for saved presets).
- **Open groups:** remembered in `localStorage` (`studio.openCategories`).
- **Tags:** tags are kept, editable on saved presets and searchable. `All genres` is a tag on Default.

`config.default_preset` can be a saved id or a built-in id (`genre:*`, `fx:*`). Built-ins only exist in JS, so **Reset all with a default preset happens client-side**, and the server's `/api/reset` is only the fallback.

### Legacy effects (`main.py`, `effects/`, `app/hyperion.py`, `app/gui.py`)
- **`main.py`:** reads the LED layout from Hyperion (or a classic config file), fakes the Hyperion effect API in `app/hyperion.py`, runs `effects/<name>.py` through `runpy`, and streams over JSON or protobuf. A single-column strip layout is split in half into left and right.
- **`effects/spectrum_dump.py`:** emulates GStreamer's `spectrum` and `level` elements with numpy over `soundcard`, keeping the original effect code unchanged.
- **`effects/palette.py`:** the color-list support used by both effects.

## Changes from upstream (so far)

- **Python 3 / Windows port:** GStreamer replaced by `soundcard` loopback plus numpy emulation. The layout is read from Hyperion.NG instead of `hyperion.config.json`, Hyperion.NG JSON is accepted (short origin, clear by priority), and there are `run.bat` and `--list-devices`.
- **Legacy effects:** palette support and a Deep Night color set (deep blue, navy, UV, pink, magenta, purple, violet, indigo).
- **FOLO studio** (formerly Audio Studio): the `studio.py` server, engine, config store and web UI.
  - **Editing:** spectrum and strip editors, inspector, live meters and a perceptual brightness bar.
  - **Triggers:** Level and Hits.
  - **Libraries:** reactive presets, 12 whole-strip effects, and 14 genre presets with tags, all in a left-hand effects menu grouped under umbrella categories with search.
  - **Your presets:** save, tag, edit and delete them, and a ★ default restored by Reset all.
  - **Colors by range:** bulk recoloring per spectrum range.
  - **Live sync:** pages follow changes made elsewhere, LED count changes are followed with segment scaling, and Undo is offered for whole-strip changes.
  - **LedFx effects:** six moving modes (scroll, drops, scan, wipe, flow, fire) with per-segment speed and size, and an LedFx group of 18 effects (Energy, Scroll, Wavelength, Power, Rain, Spotlight, Scan, Scan multi, Strobe, Blade Power+, Bar, Multicolor bar, Bands, Equalizer, Blocks, Fire, Lava lamp, Marching).

## Conventions

- **Testing:** never test on a live config or real LEDs. Use the port-8778 copy with output off (see Commands), and afterwards confirm the live `studio.json` is untouched.
- **Colors:** the built-in looks use the **Deep Night** palette: `#0033CC` deep blue, `#000080` navy, `#4B00FF` UV, `#FF3399` pink, `#FF00FF` magenta, `#800080` purple, `#8F00FF` violet, `#4B0082` indigo. Lows are blue (`LOWS_PAL`), mids pink (`MIDS_PAL`) and highs violet (`HIGHS_PAL`).
- **Outside the repo:** say what you'll change before touching Hyperion settings, WLED state or anything else outside this folder. Hyperion's settings DB (`%USERPROFILE%\.hyperion\db\hyperion.db`) may only be edited while Hyperion is stopped, after a backup.
- **Docs:** this file is the single agent guide (CLAUDE.md imports it). Update it and the README's FOLO studio section whenever features change.
- **Machine-specific notes:** a particular setup's hardware, network, live state and owner preferences go in `AGENTS.local.md` (git-ignored, imported for Claude Code by `CLAUDE.local.md`). Read it when it exists.

## Troubleshooting

- **Dark strip with Hyperion and the studio both fine:** check for audio first. Segments with no idle glow (`floor` 0) render black in silence, and the stream's `spectrum` sits at −120 when nothing plays.
- **Hyperion's own audio effect or a pinned source hides the studio:** these API commands work without auth: `{"command":"sourceselect","auto":true}` and `{"command":"componentstate","componentstate":{"component":"AUDIO","state":false}}`. `serverinfo` → `priorities` shows which source is visible.
- **WLED goes dark when Hyperion stops:** with "stay on after streaming" off, re-enable it with `POST http://<wled-host>/json/state` and `{"seg":[{"id":0,"on":true}]}`.
- **Stopping the studio:** it runs as two `python.exe` processes whose command line contains `studio.py` (the venv launcher plus the interpreter); stop both.

## Not tested yet

- Studio shutdown with Ctrl+C.
- Switching audio devices while the studio runs.
- Legacy `main.py --gui` and protobuf/matrix mode.
