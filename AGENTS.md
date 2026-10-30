# AGENTS.md

Guidance for AI coding agents (Claude Code, Codex and others) working in this repository.

## Source of truth: `docs/`

The [`docs/`](docs/README.md) folder holds FOLO's decisions (ADRs), specifications, architecture, run books and process. **It is the source of truth; the software exists as the compliant implementation of it** ([ADR-0001](docs/decisions/ADR-0001-documentation-is-the-source-of-truth.md)).

- FOLO is being rebuilt to the target described in `docs/` (see the [roadmap](docs/roadmap.md)). Until each milestone lands, the rest of this file describes the **current** code.
- **Before changing behaviour,** read [docs/README.md](docs/README.md) and the relevant ADRs and specifications. Change the documents first, following [docs/process/workflow.md](docs/process/workflow.md).
- **Implementation work** follows the [run books](docs/runbooks/README.md).
- **Real devices:** anything that touches them follows [docs/process/testing-policy.md](docs/process/testing-policy.md).
- **Writing docs:** no time estimates and no personal details.
- **Branches:** rebuild work goes to `v1`, the default branch ([ADR-0030](docs/decisions/ADR-0030-rebuild-on-a-v1-branch.md)). `main` is the frozen legacy studio: no merges and no fixes.
- **Branches, commits and pull requests:** `<type>/<short-topic>` branches, Conventional Commits and the PR template's documentation impact section, summarised in [CONTRIBUTING.md](CONTRIBUTING.md).

## Reviewing documents with EGM

The [Elephant-Goldfish Model](https://github.com/google/elephant-goldfish-model) (EGM, the `egm` plugin) checks a document with reviewer subagents that start with no context and read only the document and the code. Use it where it fits the docs-first process above:
- **Install:** in Claude Code, `claude plugin marketplace add google/elephant-goldfish-model` and then `claude plugin install egm@egm`. In Codex, `codex plugin marketplace add google/elephant-goldfish-model` and then `codex plugin add egm@egm`.
- **Before a document's pull request:** run `/egm:egm-review <document>` on a new or changed ADR, specification or run book, then `/egm:egm-resolve` to work through the findings. In Codex the commands have no `egm:` prefix.
- **A new feature or decision:** `/egm:egm-brainstorm`, then `/egm:egm-draft`. EGM writes to `docs/designs/` by default, which isn't one of FOLO's documents. Move the result to where FOLO keeps it: an ADR from [the template](docs/decisions/adr-template.md), a specification in `docs/specs/`, or a run book step.
- **Documents against the code:** `/egm:egm-drift-report` on `docs/specs/` or `docs/architecture/` lists where the code has moved away from them.
- **Not `/egm:egm-bootstrap`:** it writes `README.md` files across the tree, and FOLO's documentation lives in `docs/`.
- **Who runs them:** a person. The skills can't be started by an agent (`disable-model-invocation`), so agents suggest them at these points. Their findings are input; decisions still go through [the workflow](docs/process/workflow.md).

## What this is

**FOLO** (formerly Vibearion): a Windows / Python 3 fork of RanzQ/hyperion-audio-effects (2017, Python 2, Linux GStreamer). The main product is the **FOLO studio** ("Fear of lights out", formerly the Audio Studio): a local web UI where LED segments are mapped to frequency ranges and rendered live to [Hyperion.NG](https://github.com/hyperion-project/hyperion.ng), which drives a WLED strip. The upstream effect runner (`main.py` + `effects/`), ported to Python 3, is gone from `v1` ([ADR-0023](docs/decisions/ADR-0023-legacy-code-and-licensing.md)); it stays on `main` and in the `v0.9.0-legacy` tag.

No build step for the studio. Tests, lint and type checks run through uv and cargo, and CI runs them on Windows and Ubuntu (`.github/workflows/ci.yml`). The contracts of the rebuild (`specs/`) generate committed Python, Rust and TypeScript code; CI fails when it's out of date (see [Contracts](#contracts-specs-rb-02)).

## Commands

Everything runs from the project's venv (Python 3.13). `uv sync` creates or updates it with the locked runtime and dev dependencies (`pyproject.toml`, `uv.lock`). It manages `.venv`, the venv `studio.bat` uses, so a running studio needs a restart afterwards. Without uv, the runtime alone:

```
py -3.13 -m venv .venv
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

| Task | Command |
|---|---|
| The effect engine for a node (RB-04; the node starts it with `FOLO_SOURCE_LINK` and `FOLO_SOURCE_TOKEN` in its environment) | `uv run folo-source [--seed N]` or `python -m folo_source` |
| Migrate a v1 `studio.json` to the v2 documents (RB-05; keeps `studio.json.v1.bak`) | `uv run folo-source migrate-v1 <studio.json> <out-dir>` |
| Run the studio (opens http://127.0.0.1:8777/) | `studio.bat` or `.venv\Scripts\python.exe studio.py [--port 8777] [--no-browser] [--config studio.json] [--seed N]` |
| List audio sources (outputs, inputs, apps playing audio) | the studio's Audio input card, or `GET http://127.0.0.1:8777/api/sources` |
| Tests (goldens, beat fixtures, `sanitize_config`, the studio starting, the clamper, contract drift and the conformance harness; about two minutes) | `uv run pytest` (`--hypothesis-profile=thorough` tries 5,000 inputs per property) |
| Lint, format, types | `uv run ruff check .`, `uv run ruff format .`, `uv run pyright` |
| Every pre-commit hook | `uv run pre-commit run --all-files` (`uv run pre-commit install` once to run them on commit) |
| Regenerate the goldens (only when behaviour is meant to change) | `uv run python scripts/make_golden.py [--export-builtins] [--only renderer,beat,corpus,clamp,migration,hyperion]` |
| Contract tools (once, needs Node.js 22) | `npm ci` in `ui\` |
| Regenerate the schema bundle and the Python, Rust and TypeScript types (after changing `specs/`) | `uv run python scripts/contracts.py generate` |
| Validate the contracts and fail on drift (what CI runs) | `uv run python scripts/contracts.py check`, then `npm run check:types` in `ui\` |
| Conformance: an API, or captured WebSocket messages | `uv run python scripts/conformance.py api --node target/debug/folo` (starts that node on a home and port of its own, as CI does) or `--url <base>/api/v1` (never a live setup) / `messages [DIR]` (default `specs/fixtures`; `target/tmp/live-messages` after `cargo test` holds what the node's live channel sent) |
| Rust | `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (toolchain pinned in `rust-toolchain.toml`; on Windows the MSVC host, which needs Visual Studio's C++ build tools). `FOLO_E2E=1` with `folo-source` on the `PATH` (an activated venv) adds the node's test with the real engine. |
| Run the node (RB-06, RB-07; it starts `folo-source` from beside itself or the `PATH`, and serves the client API at `http://127.0.0.1:<port>/api/v1`) | `cargo run -p folo-node -- --home <folder> --port 8778` (a home of its own beside a live setup); `folo --help` lists the flags. Run from the repository root, its first run imports the live `studio.json` ([ADR-0034](docs/decisions/ADR-0034-the-node-process.md) D10), read only, and leaves a `studio.json.v1.bak` copy beside it. |
| The node's status, or apply `node.toml` and the documents to it | `cargo run -p folo-node -- status [--json] --home <folder>` / `reload --home <folder>` |
| The built-in packs (RB-09; generated from `web/studio.js`, needs Node.js 22): regenerate after changing a built-in, or check them (what CI runs) | `uv run python scripts/build_packs.py generate` / `uv run python scripts/build_packs.py check`; `FOLO_NODE_TESTS=1 uv run pytest tests/test_builtin_packs.py` runs the builders in Node |
| The clean-room rewrite gate (fails while a LedFx-inspired item isn't rewritten) | `FOLO_RELEASE_GATE=1 uv run pytest tests/test_clean_room_gate.py` |
| Frozen studio (PyInstaller one-dir) | `uv sync --group packaging`, then `uv run pyinstaller --noconfirm packaging/folo-studio.spec`; `dist\folo-studio\folo-studio.exe --help` |
| Syntax check the UI | `node --check web\studio.js` |
| Ask Hyperion for its LED layout | JSON API: send `{"command":"serverinfo","tan":1}\n` to `localhost:19444` (see `folo_source.legacy.json_client.fetch_server_info`) |

**Restart rules:** Python changes need the studio process restarted (Ctrl+C in its console, or kill the `python.exe` running `studio.py`, then start `studio.bat` again; LEDs blink briefly). `web/*` files are served with `no-store`, so UI changes only need a page refresh.

**Testing without touching the live setup:** `studio.json` is the live config, edited through the UI, and the studio drives real LEDs. Test destructive or experimental changes on a second instance with a copied config and Hyperion output disabled:

```
# copy studio.json elsewhere, set "hyperion": {"enabled": false, ...} in the copy, then
.venv\Scripts\python.exe studio.py --port 8778 --no-browser --config <copy>.json
```

Engine logic can be tested offline by importing `folo_source.inputs.audio.analyzer.Analyzer` / `folo_source.legacy.renderer.Renderer` (or `folo_source.surfaces.renderer.SurfaceRenderer` with v2 layers) and feeding numpy blocks, frames × channels or mono (see how `Renderer.render(config, spectrum, dt)` is called from `StudioEngine._on_packet`, after `AudioInput.analyse`). `tests/golden_cases.py` does exactly that for the golden frames, with synthetic signals from `tests/synth.py` and `Renderer(analyzer, rng=np.random.default_rng(seed))`. For A/B work on real audio, record the capture device to a `.npy` (`device.recorder(samplerate=48000).record(numframes=...)`) and feed both engines the same 480-frame packets so their frames line up. Stacking the frames of one segment into an image (time down, LEDs across) is a quick way to see whether a moving mode looks right. A speed-up should leave the output byte for byte the same, which `tests/test_golden_renderer.py` checks on 279 cases (every mode × direction × trigger, gains up to +54 dB, the ambient light, every built-in look). For a deeper A/B, export the committed package (`git archive HEAD source | tar -x -C <dir>`), run it in a second process with that `source` folder first on `sys.path`, feed both the same packets with the renderers' `rng` seeded alike, and compare frames, levels and spectra, including silence, gains up to +54 dB and every mode. Browsers throttle timers in hidden tabs to about 1 s, so in-page JS checks need multi-second waits.

**Testing app capture without sound:**
- **Tone:** play an inaudible tone (18.5 kHz at −70 dBFS) from a process with a distinctive name. A copy of the base interpreter renamed `tonegen.exe` works, run with `PYTHONHOME` set to the base install and `PYTHONPATH` to the venv's site-packages.
- **Check:** "only" that app must show the tone, and "except" it must show silence (−120 dB).
- **Time:** `time.process_time()` ticks in 15.6 ms steps on Windows, so CPU comparisons need long or repeated runs.

**Testing the beat without sound:**
- **Offline:** synthesize songs with known beat times (kick, snare, hats, a bass line and a chord per bar, at 60–180 BPM, four-on-the-floor, rock, swung hip-hop, tempo drift, breakdowns), feed `Analyzer` and `BeatTracker` the same 480-frame packets, and score the fired beats against the true ones (±70 ms) plus the bar starts. Also feed music without a steady beat (pads, random piano notes, speech-like bursts): it must never lock. `tests/beat_fixtures.py` has these fixtures and pins what the tracker does on them. Its known defects (the bar position on rock patterns and after breakdowns, locking on perfectly clean tones) are listed in `docs/architecture/current-system.md`, and their tests are expected to fail.
- **In the page:** run `studio.py` with `folo_source.inputs.audio.windows.audio_input.Capture` replaced by a fake that streams synthetic songs at real-time pace (no sound plays), then check the tempo pill, the Music card and beat segments.
- **Player controls:** don't press play/pause, next or previous while testing: they act on the owner's real player.

`.claude/launch.json` (git-ignored) has a `studio-test` configuration for the in-app browser preview: port 8778 with a config copy in a session scratchpad. Point `--config` at your own copy before using it.

`paseo.json` sets up a new Paseo worktree: it copies the git-ignored `AGENTS.local.md`, `CLAUDE.local.md` and `.claude/launch.json` from the checkout the worktree came from, then runs `uv sync` and `npm ci` in `ui\`.

## Architecture

### Studio server (`studio.py`)
`ThreadingHTTPServer` bound to 127.0.0.1. It refuses requests whose `Host` isn't local, and POSTs that aren't `application/json`.
- `GET /` serves `web/`
- `GET /api/state`: config, audio devices, apps with audio, display constants
- `GET /api/sources`: just the devices and the apps with audio (the Audio input card refreshes it every 5 s)
- `GET /api/stream`: server-sent events at 30 fps
- `GET /api/art`: the album art of what's playing (the page adds `?<art id>` so a new cover isn't cached)
- `POST /api/config`: the page always posts the **whole** config
- `POST /api/reset`
- `POST /api/layout`: re-read the LED count from Hyperion
- `POST /api/media`: `{"action": "play_pause" | "next" | "previous"}` on the player shown as now playing

A watcher thread polls Hyperion's LED count every 5 s and calls `ConfigStore.set_led_count`, which scales all segments proportionally.

The server runs on its own thread while the main thread waits for a stop:
- **Logs:** JSON lines on stderr (`runtime/logs.py`): `ts`, `level`, `logger`, `msg` and `exc` (the format in [control plane](docs/architecture/control-plane.md#supervision)). Nothing prints.
- **Stopping:** Ctrl+C, Ctrl+Break, closing its console or SIGTERM (`runtime/lifecycle.py`'s `Shutdown`). It stops the server, the engine and the layout watcher, joins their threads within 3 s, clears its colors in Hyperion and saves the config. A kill still works: Hyperion releases the LEDs after `duration`.
- **Randomness:** `--seed N` seeds the random modes (drops, fire), for repeatable output.

### Config (`folo_source/legacy/v1.py`)
`sanitize_config()` is the single source of truth for the schema and its limits. Everything the UI sends goes through it. It stays as the reference for the v1 migration and the clamp corpus. The file is read and written as UTF-8.

`ConfigStore` behaviour:
- **Replacement:** the config dict is replaced as a whole on every change, never mutated, so readers use it without locks.
- **Saving:** writes to `studio.json` (git-ignored, per-machine state) are debounced.
- **Version tags:** `revision` and `server_id` let open pages detect outside changes and reload.
- **Stale posts:** a POST carrying an old `led_count` gets its segments scaled to the current count.
- **Reset:** `reset()` keeps `audio`, `hyperion`, `reactive`, `ambient`, `beat`, `now_playing`, `presets` and `default_preset`.

Audio reactiveness and the ambient light (top level, not part of presets):
- `reactive`: `on` (the segments follow the audio), `auto` (the ambient light after `ambient.idle_after` seconds of silence) or `off` (the ambient light only).
- `ambient`: `color` (hex), `style` (`solid` or `breathe`), `level` (output scale 0–1 like `master`) and `idle_after` (1–600 s).

The music, also top level:
- `beat`: `offset`, ms (−200…200) the beat triggers fire before (+) or after the beat is heard in the captured audio, to make up for the LEDs' delay.
- `now_playing`: `enabled` (follow Windows' media sessions), `cover_colors` (the segments take the album art's colors) and `pause_ambient` (in `auto`, the ambient light comes on as soon as the player is paused).

Segment fields:
- `start`, `end`: LED indices
- `freq_lo`, `freq_hi`: Hz
- `trigger`: `level`, `hits` or `beat`
- `beats`: the beat trigger flashes every 0.5, 1, 2 or 4 beats, lined up with the bar
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
- **`beat`:** dB levels like `level`. They set how bright each beat is from the band's level, and a beat stays dark below the threshold.

The JS `enforce()` / `dbLimits()` must stay in sync with these limits.

### Engine (`source/folo_source/`, the `folo-source` package)
The effect engine, restructured from the studio's engine in RB-03 with the same output (the goldens pin it). `studio.py` puts `source/` on `sys.path`, so it runs without installing the package; `uv sync` installs it editable. Paths below are inside `source/folo_source/`:
- `core.py`: the pipeline's core types (RB-04 step 1): `Frame` (rgbf32 on today's 0–255 scale; `to_rgbf32()` rounds toward zero, so truncating gives today's bytes), `Signal`, `PixelFormat` and `now_us()`, the monotonic microsecond clock
- `inputs/base.py`: the `Input` interface (`kind`, `thread`, `start()`, `stop()`, `health()`, `status()`)
- `inputs/audio/analyzer.py`: `Analyzer`, `Spectrum`, `AutoLevel` and `channel_peaks()`
- `inputs/audio/input.py`: `AudioInput`, capture plus analysis (gain and auto level) into `AudioPacket`s, with `band_levels()` and the `input` notification's `status()`. The legacy `StudioEngine` runs on it.
- `inputs/audio/capture.py`: this platform's `Capture` and `sources()`, on `common.py`'s `CaptureBase` (state, stats, the meter)
- `inputs/audio/windows/`: the WASAPI capture (`Capture` in `audio_input.py`, on `wasapi.py`)
- `inputs/audio/linux/`: the PulseAudio capture (`Capture` in `audio_input.py`, on `pulse.py`, [ADR-0031](docs/decisions/ADR-0031-linux-audio-capture.md))
- `inputs/media/input.py`: `MediaInput`, now playing through `inputs/media/windows.py` (media sessions) or `inputs/media/linux.py` (MPRIS on the session D-Bus, through dbus-fast; Pillow decodes local album art); `inputs/media/art.py`: the art's size limits and `cover_colors()`
- `inputs/generator.py` (time, LFOs and noise), `inputs/screen.py` (`ScreenInput`: the newest screen frame the node forwards, stamped with its arrival on the engine clock; the newest one that is neither `held` nor `protected` is the picture, and it is active for 0.5 s after it arrived, ADR-0037 D12 and D13; frames are kept in memory only, and its `status()` holds the state, rate, frame size, flags and bars, never pixels) and `inputs/listing.py` (what `inputs.list` returns)
- `signals/beat.py`: the beat tracker, and `BeatSignal`: a tracker per zone with its beat offset, publishing `[bpm, confidence, locked, phase, bar_beat]`
- `effects/base.py`: the effect interface (`EffectInfo`, `LayerSpec`, `Effect` for effects that draw a line, `PostEffect` for those that change what is below them, `CanvasEffect` for those that draw a rows × columns picture for their layer's rectangle of a canvas, and `EffectInputs`, the engine's inputs that factories get beside the analyzer and the random generator) and the registry (`register()`, `create()`)
- `effects/screen.py`: the `screen` canvas effect (RB-14, [display capture](docs/integrations/display-capture.md#the-screen-effect)): crop to the bars the agent found (`follow`) and the margins, `area` or `edge` sampling onto the layer's rectangle (vectorised, weights from `publish/frames.py`'s `area_weights()`), saturation and brightness, then smoothing by `1 − e^(−dt/smoothing)`; it draws nothing on a strip or before a usable picture, and keeps the last picture through `held` and `protected` frames
- `effects/post.py`: the post-processing effects `brightness`, `mirror`, `blur` and `trail`
- `effects/visualizers.py`: the `audio-visualizer` effect (`AudioVisualizer`, `LayerState`): today's segment logic, the modes and the beat targets, drawing each layer's line; `effects/colors.py`: palettes and album colors
- `surfaces/renderer.py`: `SurfaceRenderer`, a surface's layer stack: regions, axis modes, blend modes, background and master, as `docs/architecture/pipeline.md` defines them (a `relative` region with `snap: nearest` rounds both ends to the nearest cell, halves up, as the studio's builders do; without `snap` it covers every cell it touches)
- `runtime/executor.py`: `Executor`, which renders the surfaces of a node's `config.apply` step by step (beat and activity first, then each stack) and publishes them, with the `spectrum` signal and the `levels` notifications when `publish` asks for them, and the `activity` of each surface with an enabled Screen layer (`screen_activity()`: active while the screen input is, or the audio when the surface has audio layers too, no `level_db`); `runtime/source.py`: `SourceEngine`, the engine as its node runs it: the inputs, the render thread (merging packets that came during a slow frame, idle rendering, and a screen frame starting a step while audio is idle, so Screen layers render at the capture rate, ADR-0037 D11), the notifications and the answers to the node's requests; `cli.py`: the `folo-source` command
- `runtime/engine.py`: `StudioEngine`, which wires them together for the legacy studio and renders silence while idle; `runtime/lifecycle.py`: shutdown, thread joins, the seeded random generator; `runtime/logs.py`: JSON-line logging
- `legacy/`: what the controller takes over later: `v1.py` (the config), `migrate.py` (the v1 migration, RB-05: the v2 documents and the "My presets" pack, the same bytes on every run), `renderer.py` (the v1 `Renderer`: a config's segments as audio-visualizer layers on one strip surface, plus the ambient light), `ambient.py` (the ambient light, RB-08), `hyperion.py` and `json_client.py` (the Hyperion output, RB-10)
- `publish/`: publishing to the node (RB-04 step 7): `frames.py` (surface and signal frames: resampling by area, quantising to rgb8, rgb16 or rgbf32, LZ4, and decoding for tests; and `decode_screen_frame()`, which checks a capture agent's `FOLC` frame: magic, version, pixel format, flags, size, length and bars), `publisher.py` (`Publisher`: each surface at its requested rate and resolution, sequence numbers, the signals asked for) and `link.py` (`LocalLink`: the local link of ADR-0032 on its own asyncio thread, with the launch token, newest-wins sending, JSON-RPC answers, reconnecting, and giving up after 30 s; the node's `0x04` screen frames are decoded on its thread and handed to the screen input, a bad one dropped with its reason logged, never its pixels). `tests/fake_node.py` is the node it's tested against.
- `contracts/`: the generated models and the clamper (RB-02)

- **Audio input (`inputs/audio/windows/audio_input.py` on `wasapi.py`):** the studio's own WASAPI capture through cffi. On Linux `inputs/audio/linux/` captures instead (see below). Elsewhere `wasapi` isn't imported, so the code still imports: there are no sources, and the input reports "Audio capture needs Windows".
  - **Linux:** `inputs/audio/linux/` implements [ADR-0031](docs/decisions/ADR-0031-linux-audio-capture.md) (the sources, following, packets and stats are decided there). `pulse.py` is the cffi binding over `libpulse.so.0` with its threaded main loop; `audio_input.py` names outputs by their description and records them through their monitor. It never autospawns an audio server, and without libpulse the input says so. `tests/test_linux_capture.py` runs it on a fake server everywhere; `tests/test_pulse_capture.py` runs it against null sinks of a real PulseAudio server, which CI's Ubuntu job starts (`FOLO_PULSE_TESTS=1` turns a missing server into a failure).
  - **Sources** (config `audio`):
    - an output's loopback (`device`: `default` or a name)
    - an input (`in:` + name)
    - only one app, or everything except one app (`app`: exe name, `app_mode`: `only` / `except`). This uses Windows 10 2004+ process loopback (`ActivateAudioInterfaceAsync` with a cffi COM completion handler) on the app's top process (`app_root()`), so its helper processes count too, whatever output it plays on.
  - **Stream:** event driven (`AUDCLNT_STREAMFLAGS_EVENTCALLBACK`, falling back to polling) in the MMCSS "Pro Audio" class. Every wake drains all queued packets (`GetBuffer` until `AUDCLNT_S_BUFFER_EMPTY`), so the analysis always runs on the newest audio, at 48 kHz float. App capture takes only the flags in Microsoft's sample: LOOPBACK, EVENTCALLBACK and AUTOCONVERTPCM.
  - **Silence:** an output's loopback sends no packets during silence, while app capture sends silent ones. After a gap of more than `GAP` (0.1 s) the batch is `fresh` and the analyzer window is cleared.
  - **Following:** `default` follows Windows' default output through `IMMNotificationClient` notifications (`wasapi.device_changes()`), plus a 10 s safety poll, because asking for the default costs 2 ms. An app is re-attached when its top process changes, waits while it isn't running (state `waiting`), and "except" listens to everything until the app starts. A lost device (`AUDCLNT_E_DEVICE_INVALIDATED`) is reopened after 0.5 s.
  - **Stats** (`Capture.status()`): packets/s, the median delay from WASAPI's QPC stamp to the engine (about 2 ms for loopback, about 10 ms for app capture, which stamps the packet's start), discontinuities (`glitches`), and a falling peak meter of the first two channels with a clip flag. The per-channel peaks are computed once and passed to `Analyzer.push()` for its active-channel detection.
- **Input gain and auto level:** `Analyzer.offset` (dB) is added to every level: `audio.gain`, plus `AutoLevel.gain` when `audio.auto_level` is on.
  - **How auto level works:** `AutoLevel` follows the loudest bin without gain (`Spectrum.loudest`) with a 300 ms rise and an 8 s fall, holds during silence, and adds what puts it at `AUTO_TARGET_DB` (−15), between −12 and +30 dB.
  - **Silence detection** (`Ambient`, auto level) uses the level without gain, so gain never makes silence look like sound.
- **`Analyzer`:** Hann-windowed FFTs of the latest samples, returned as a `Spectrum`. A band's level is the loudest bin in its range, interpolated when the band is narrower than one bin (`band_levels()`, vectorised with `np.maximum.reduceat` over index plans: `band_plan()` for contiguous edges, `bands_plan()` for separate, possibly overlapping bands, read by `planned_levels()`).
  - **History:** new samples are appended to a buffer of `HISTORY` (2 × 4096) per channel, and the latest 4096 move back to the front only when it fills, instead of shifting the whole window on every packet.
  - **Quiet windows:** while every sample of the active channels in the window is too quiet to lift any bin above −120 dB at the current gain (`QUIET_PEAK_DB`, from the window's sum bound with 1 dB of margin), `push()` returns the silent spectrum without FFTs. Paused app capture sends 100 silent packets a second, and these cost almost nothing now. The result is the same as the FFTs would give.
  - **Two windows:** `full` uses 4096 points (11.7 Hz bins, 85 ms). It drives the UI's spectrum and level triggers. `fast` uses 2048 points and drives hits triggers, including spectrum mode on hits. Measured on real music, hits light 10–20 ms sooner (a mixed-in snare burst: 16.7 → 10 ms), with the same share of time lit. `fast` is only computed when a hits segment asks for it.
  - **What hits report:** hits segments report the `full` level and its own running average (`shown_average`) in `levels`, so the UI's lines still sit on the spectrum it draws.
  - **Channels:** the power of every channel that has carried sound (louder than −100 dBFS, sticky) is averaged. The capture device is 5.1, but only FL/FR carry audio, and the other four hiss at about −190 dB rather than being digitally silent. The old code averaged all six channels into mono, which cancelled out-of-phase stereo content and read 9.5 dB low.
  - **dB scale:** `CALIBRATION_DB` (−9.54 dB) keeps the scale the presets were tuned on, whatever the device's channel layout. A full scale 1 kHz sine on L+R reads −10.2 dB before and after. Compared frame by frame, readings are never lower than the old engine's: typically +1 dB, and more where stereo content used to cancel.
- **Per configuration:** `SurfaceRenderer.prepare()` and `AudioVisualizer.prepare()` work out what only changes with the config (regions, visibility, band plans of every layer, layouts, static gradients, the background) when `compose()` gets a surface or layers object it hasn't seen. This relies on the store replacing the config, never mutating it. Every layer's level then comes from one `reduceat` per window, and single values are shaped in plain Python (`shape_one()`, `color_at()`), because numpy costs microseconds per call even on one number.
- **Per layer (segment):**
  1. Band level, plus a running average (`AVERAGE_MS`) used by hits.
  2. Shaping: threshold maps to 0, ceiling to 1, then gain, then the `curve` exponent. Beat segments (`_beat_target()`) shape the band's level held at its peaks (falling `BEAT_HOLD_FALL`, 12 dB/s), and only for a moment after each beat of their grid (`BeatTracker.fired(beats)`): at least `BEAT_PULSE` (40 ms, so a frame Hyperion gets shows it) or three times the attack. Else their target is 0.
  3. Attack/release smoothing.
  4. The mode, laid out by direction, using a step-index layout where mirrored directions get half the steps.
  5. Its line is blended into the surface (max for segments), then the whole is multiplied by `master`.
- **Moving modes** (`MOTION_MODES`, after LedFx effects) keep per-layer state in `LayerState` and draw RGB per step themselves:
  - `scroll`: the color of the moment (palette by level) streams away at `speed`; the trail fades out over `width`.
  - `drops`: each hit spawns a ripple at a random step that grows at `speed` (0 = still spot), `width` wide, fading with `release`.
  - `scan`: an eye `width` long bounces between the ends at `speed` × level.
  - `wipe`: each hit sweeps the next palette color across at `speed`, eased out and turning around each time; under 100% `width` a bar travels instead.
  - `flow`: sine-wave palette blobs `width` long drift at `speed`, pushed faster by the level.
  - `fire`: base sparks heat the segment, heat rises at `speed`, and flames reach higher with the level.
  - **Hits** for drops and wipe: the shaped target rising past `HIT_ON` (0.5) after dropping below `HIT_OFF` (0.25), at most one per `HIT_GAP`. On the beat trigger every loud enough beat is a hit.
  - **Idle glow** (`floor`) is the minimum brightness of what the mode draws (scan eye, wipe colors, flow, flame height); scroll and drops use it as a dim palette backdrop.
- **Ambient light (`legacy/ambient.py`'s `Ambient`, in the v1 `Renderer.render` before `master`):**
  - **Blend:** crossfades the segments' frame with `ambient.color × level`; `breathe` pulses between 30% and 100% over `BREATHE_PERIOD` (8 s).
  - **Silence:** the audio counts as silent while its loudest bin is below `QUIET_DB` (−70 dB), and `quiet` counts the seconds.
  - **Fades:** into the ambient light over `AMBIENT_FADE_IN` (1.5 s), eased; back to reactive in `AMBIENT_FADE_OUT` (0.15 s), so the first beat after a pause still shows.
  - **Paused player:** with `now_playing.pause_ambient`, Auto switches after `PAUSE_QUIET` (0.3 s) of silence instead of `idle_after` while the player says it's paused. Other sound (a game) keeps it reactive.
- **Beat tracker (`signals/beat.py`, `BeatTracker`):** pushed with every spectrum right before it's rendered (`StudioEngine._track_beat()`), silence included.
  - **Onsets:** 100 per second: how much 24 log-spaced bands (40 Hz–10 kHz, their loudest bin, floored 50 dB below the loudest band) rose over 20 ms, averaged.
  - **Tempo:** every 0.1 s, the autocorrelation of the last 8 s of onsets (a 1 s moving average taken out, older onsets weighted down over 3 s) at each candidate period from 60 to 200 BPM, plus its double and quadruple, weighted by a log-Gaussian around 120 BPM and towards the tempo already found. A far tempo must persist 1 s before it's taken.
  - **Lock:** `confidence` is the autocorrelation at the beat, smoothed. Beats fire only after it stays above `LOCK_ON` (0.3) for 0.5 s, and stop below `LOCK_OFF` (0.2) or after 1 s of silence; 3 s of silence forgets the tempo. Synthetic music with a steady beat measured 0.35–0.8. Pads, random piano notes and speech-like bursts stayed below 0.3, apart from peaks (up to 0.32) shorter than the half second a lock needs.
  - **Clock:** `pos` counts beats and only moves forward. It runs at the tempo plus a correction (at most ±25%) that makes up the phase error over 0.8 s, so no beat fires twice or is skipped. The phase comes from a comb of beats over the last 4 s of onsets. Beats fire `lead` early: `ONSET_DELAY` (25 ms, how far the onsets lag the sound) plus `beat.offset`.
  - **Bars:** each beat of four collects accents: bass onsets (below 150 Hz) and how much the bands below 4 kHz changed since the beat before (chords and bass notes change on the first beat, drums repeat). The beat with the most (15% margin) is `downbeat`.
  - **When it runs:** only while an enabled segment uses the beat or a page streamed in the last 3 s (`_beat_wanted()`); otherwise `skip()`. A new song from now playing resets it.
  - **Measured:** on synthetic songs it locks after about 4 s and every beat lands within 10 ms. It costs about 50–70 µs a packet (0.5–0.7% of a core) while it runs.
- **Now playing (`inputs/media/windows.py`, `NowPlaying` thread):** Windows' media sessions (`GlobalSystemMediaTransportControlsSessionManager`) through the `winrt` packages; without them its state is `unavailable`.
  - **Session:** a playing one first, and among those the app the studio captures (`audio.app` in only mode, matched in the app id: Arc reports `Arc.exe`).
  - **Following:** the sessions' change events wake the thread, plus a check every second, and the title is read again every 5 s anyway.
  - **Album art:** read when the song changes (again after 2 s if the player hasn't set it yet), decoded to 48×48 by `BitmapDecoder`. `cover_colors()` picks up to five hues from a histogram weighted by chroma², at full brightness and a bit more saturated; grey or dark art gives none.
  - **Album colors on the LEDs:** `AudioVisualizer.prepare(layers, colors)` swaps each layer's palette for as many art colors (`cover_palette()`: the strongest first for lows, the second for mids, the third for highs), so a new cover is worked out once.
  - **Controls:** `control()` runs `try_toggle_play_pause_async` / `try_skip_next_async` / `try_skip_previous_async` on the shown session.
  - **Linux (`inputs/media/linux.py`, for the effect engine; the legacy studio doesn't use it):** the MPRIS players on the session D-Bus through dbus-fast, a playing one first. `PropertiesChanged` and `NameOwnerChanged` signals wake it, plus a check every second. Art is read only when the player names a local file (`file://`), decoded by Pillow. The controls call `PlayPause`, `Next` and `Previous`. Without a session bus the state is `unavailable`. `tests/test_mpris.py` runs it against a fake player on a private `dbus-daemon` (`tests/fake_mpris.py`), which CI's Ubuntu job installs (`FOLO_DBUS_TESTS=1` turns a missing daemon into a failure).
- **Idle thread (`runtime/engine.py`):** after `IDLE_AFTER` (0.1 s) without an audio packet, it renders silence at `IDLE_FPS` (30), so fades, releases and the ambient light keep going. Packets stop when nothing plays on some devices, or when capture fails. Rendering from both threads is serialised by `_render_lock`. `fps` counts audio packets only, so it reads 0 while idle. `start()` renders a first frame itself, so a page that connects at once never gets an empty one.
- **`HyperionSender` thread (`legacy/hyperion.py`):** sends JSON `color` commands with priority 100, origin `Audio Effects` and `duration` 2000 ms, so Hyperion releases the LEDs if the studio dies. Hyperion rejects an `origin` longer than 20 characters.
  - **Rate:** it checks for a new frame at the configured fps and sends it when it changed. An unchanged frame (silence, a solid ambient light) goes out again only every `KEEPALIVE` (0.5 s), well within `duration` and WLED's 2.5 s live timeout. Parsing our 40 commands a second cost Hyperion's JSON thread about 6% of a core.
- **Stream snapshot fields:** `spectrum` (192 log-spaced bands, 20 Hz–20 kHz, as dB integers), `leds` (hex), `levels {id: [raw dB, value 0–1, average dB]}`, `led_count`, `revision`, `server_id` and `status`. The bands and the rounded levels (`shown_levels()`; `render()` returns them unrounded) are worked out once per streamed frame and shared by every open page. `status.ambient` is the fade (0 reactive … 1 ambient light) and `status.quiet` the seconds of silence. `status.input` is `Capture.status()` plus `gain` and `auto` (the auto level gain), and `status.audio.device` is the source's label. `status.beat` has `bpm` (0 until found), `confidence`, `locked`, `phase` (0–1 through the beat) and `bar_beat` (0 is the first). `status.media` is `NowPlaying.status()`: `state` (`off`, `unavailable`, `error`, `none`, `playing`, `paused` or `stopped`), `app`, `title`, `artist`, `album`, `art` (an id, 0 without art) and `colors`.

### Web UI (`web/studio.js`, `index.html`, `studio.css`)
A single-file vanilla JS app with no framework or bundler, and no CDN, so it works offline.

**Edit flow:**
1. The page keeps the full `config`, and every edit mutates it then calls `commit()`.
2. `commit()` is a debounced POST of the whole config.
3. The server's sanitized response replaces `config` only if no newer edits happened in the meantime (`editSeq`, `pendingSave`, `saving`).
4. The stream triggers `reloadFromServer()` when `revision` or `server_id` moved and this page has no pending edits.

**Drawing:** `frame()` redraws the canvases, meters and status from the latest stream data. It runs only when something calls `invalidate()`: a stream message (30 per second), user input (document-level capture listeners), a canvas resize (a `ResizeObserver`) or `applyState()`. Anything else that changes what a canvas shows must call `invalidate()` too.
- **Keep the page cheap:** the page used to cost Chrome about 2.2 cores, and these rules brought it to about 0.6. Measured on a 1920×1080 window, 144 Hz display.
  - **No idle work:** drawing on every display refresh, or leaving any CSS animation running, makes the browser render the whole page at the display's full rate.
  - **Previews:** the effects menu previews and reactive tiles therefore loop only while hovered or focused; at rest they look as they do with reduced motion.
  - **Live bars:** live meter bars move by `transform` only (`setMeter()`), never `width`/`left`, so they skip layout.
  - **Static layers:** the analyzer grid, the strip's LED shapes and the track's segments (everything but their level bars, keyed by the segments, rows, selection and drag) are cached offscreen (`cachedLayer()`). The track's gradients, clips and shadowed labels cost 0.7 ms a frame before; now it's 0.05 ms.
  - **LED colors:** these are one row of pixels stretched over the shapes (`ledRowImage()`), decoded from hex with a lookup table (`hexToBytes()`, 4× faster than `parseInt`).
  - **Hidden pages:** a hidden page closes its stream (`followVisibility()`) and opens it again when shown, so a background tab stops parsing 30 messages a second.
- **Analyzer:** segment boxes, with hits boxes positioned at the live average plus their offset.
- **LED strip and lane track:** drag to create, move or resize segments. The bar above the track has + Lows, + Mids, + Highs and Delete (`addKind()`, `deleteSelected()`). A new segment copies the settings of the last segment of its kind (`kindOf()`: by name, else by the band's center; `kindDefaults()` when there is none), takes the lowest free number (Mids 2 after Mids 2 was deleted), and goes into the longest free run of LEDs, or else the right half of the selected (or widest) segment. It is listed in strip order. Add and delete both toast an Undo (`undoToast()`).
  - **Rows:** `computeLanes(last)` places segments with a `lane` in that row and the rest (−1) in the first row where they fit. A segment that would overlap another in its row moves down (`last` is the one to move), and empty rows close up. Dragging a segment up or down moves it to another row, or to a new one via the empty row at the bottom. Dragging on empty space creates a segment on that row. After every drag `pinLanes()` / `settleLanes()` store the displayed row of every segment. Overlapping segments in different rows still max-blend on the LEDs.

**Look:** glassmorphism.
- **Panels:** cards, the top bar and the toast are translucent `--glass` with a light edge (`--glass-line`), a top highlight (`--glass-shadow`) and `backdrop-filter: var(--glass-blur)` (blur plus saturation).
- **Background:** the panels sit over the Deep Night glows on `body::before`, a fixed layer of its own, so scrolling never repaints it. Don't go back to `background-attachment: fixed`, which repaints on scroll.
- **Controls:** inputs, switches, meters and previews are sunk into the glass (`--well`). Rows and tiles are light glass (`--panel-2`, hover `--hover`). Don't nest backdrop filters.
- **Cost:** the panel blur costs Chrome's GPU process about 8–10% of a core, because the canvases inside blurred panels redraw 30 times a second.

**Layout:** three grid areas: the effects menu on the left (`aside#presets`, sticky below the top bar via the `--topbar-h` variable), the editor (`.stage`) in the middle, and the inspector (`.side`) on the right. Under 1500 px the inspector moves below the editor; under 1100 px everything stacks.

**Panels:** the effects menu, segment list, inspector (built from `rangeField` / `segmented` helpers that register `syncers`), Colors by range, Reactive presets, Ambient light, Music, Audio input and Settings. The inspector shows Speed and Size only for moving modes, and names Size after the mode (`SIZE_LABEL`). With the Beat trigger it shows "Flash every" (`BEAT_EVERY`); switching triggers converts the thresholds only to and from hits.

**Music card and top bar** (`renderMusic()`, `syncMusic()`, `updateMusic()`, `showMedia()`):
- **Top bar:** a now-playing chip (art, title, artist; dimmed when paused) that opens the Music card, and a tempo pill whose dot flashes on the beat (pink on the first of the bar).
- **Card:** the art, title, artist and player, previous / play-pause / next, Album colors (with the art's colors as swatches), On pause, Players (reading them at all), the four beats of the bar, the tempo and the Beat timing slider (double-click resets it).
- **Cheap:** what's playing is rebuilt only when `status.media` changes; the beat dots change only their opacity (`setOpacity()`), driven by `status.beat.phase` on each stream message. No CSS animation.

**Audio input card** (`renderAudioInput()`, `syncAudioInput()`, `updateAudioInput()`):
- **Listen to:** a select whose values are `out:` / `in:` / `only:` / `except:` + name, in groups: everything you hear, only one app (with "playing"), everything except one app, and inputs.
- **Status:** format, packets/s, delay and glitches.
- **Meters:** L/R meters (−60…0 dBFS, a cover moved by `transform`) with a CLIP light.
- **Gain:** ±24 dB; double-click resets it.
- **Auto level:** a switch, showing the gain it adds right now.

**Audio reactive:** an On / Auto / Off switch in the top bar and the Ambient light card (`renderAmbient()` / `syncAmbient()`) both set `config.reactive`.
- **Card:** color (the eight Deep Night colors or a custom one), Solid / Breathe, a perceptual brightness slider and, in Auto, the "switch after" delay (`IDLE_STEPS`).
- **Live state:** `updateAmbientStatus()` says what the strip shows, from `status.ambient` / `status.quiet`, and lights the top bar's dot in the ambient color. Clicking the dot opens the card.

**Collapsing:** every card with `data-collapse="<key>"` gets an arrow button in its `h2` (`setupCollapsibles()`). A collapsed card (`.collapsed`) shows only its title, and the state is remembered in `localStorage` (`studio.collapsedCards`). `data-collapsed` makes a card start collapsed (Settings). The inspector card wraps `#inspector`, which is rebuilt on every selection, under a static "Selected segment" head. Canvases in a collapsed card have no size, so `frame()` skips drawing them.

**Segment defaults:** every segment builder (`part`, `rangeSegment`, `addSegment`, `autoSplit`) spreads `MOTION_DEFAULTS` (speed 120, width 20, beats 1, the server's defaults). `EFFECT_KEYS` compares speed, width and beats when detecting the active effect, so a built-in without them would never show as active.

**Brightness:** the sliders are perceptual (position^2.2). `config.master` stores the linear output scale.

**Presets library** (`library()`):
- **Saved presets:** from `config.presets`; they store `led_count` and are scaled on apply.
- **Genre presets:** from `GENRE_PRESETS`, built by `layoutRanges()`. Layouts are alternate, zones, mirror or stack. Each range names a per-segment preset from `REACTIVE`; that preset's `range` supplies trigger and thresholds, and its `response` supplies motion.
- **Effects:** custom `EFFECTS` builds from `REACT` building blocks, including the beat ones (`beatFlash`, `barWipe`, `halfTime`, `barFlash`) behind Beat sync and Half-time pulse.
- **Reactive presets:** `REACTIVE` groups, ending with "On the beat". A preset's `range` includes `beats` (`RANGE_KEYS`).
- **LedFx effects:** `LEDFX_EFFECTS`, 18 ports of LedFx's audio reactive effects (from its source, `ledfx/effects` on GitHub, since its docs only cover a few). They use LedFx's lows (30–250 Hz), mids (250–3000 Hz) and highs (3–10 kHz) as `LFX_LOWS` / `LFX_MIDS` / `LFX_HIGHS`, in Deep Night colors. Each has `source: 'LedFx'` (a badge in the menu), a `LedFx` tag and an id `fx:ledfx-<name>` (`effectId()`). LedFx's BPM effects (Bar, Multicolor bar) use the beat trigger (`onBeat()`).

**Effects menu:** the library is grouped under umbrella categories in `CATEGORIES`: My presets, LedFx, Electronic, Hip-hop & R&B, Pop & Rock, Jazz & Classical, Chill & Ambient and Visualizers.
- **Membership:** built-ins are listed in categories by id, saved presets always go to My presets, and anything left unlisted falls into a "More" group. Add a new built-in to a category's `items`.
- **Search:** matches name, description, tags, category and range names and reactions.
- **Interaction:** a click applies the effect, and ▾ expands its details (ranges, tags, ★ default, Edit for saved presets).
- **Open groups:** remembered in `localStorage` (`studio.openCategories`).
- **Tags:** tags are kept, editable on saved presets and searchable. `All genres` is a tag on Default.

`config.default_preset` can be a saved id or a built-in id (`genre:*`, `fx:*`). Built-ins only exist in JS, so **Reset all with a default preset happens client-side**, and the server's `/api/reset` is only the fallback.

### Contracts (`specs/`, RB-02)
The machine-readable forms of the prose specifications in `docs/specs/`. Nothing in the running studio uses them yet; the rebuild's engine and node will.
- **Sources:** JSON Schemas per document and entity in `specs/schema/` (including `studio-v1.json`, today's `studio.json`), `specs/openapi.yaml` (OpenAPI 3.1, the client API) and `specs/asyncapi.yaml` (AsyncAPI 3.0: the live channel, the source link, ingest and the binary frames). Clamping rules the schema keywords can't say are custom `x-folo-*` keywords, read by both clampers.
- **Generated, committed, never edited:** `specs/generated/folo.schema.json` (all schemas bundled into one `$defs`, every object default expanded), `source/folo_source/contracts/models.py` (pydantic), `crates/folo-protocol/src/generated.rs` (typify, run by `crates/xtask`), `ui/src/lib/api/schema.ts` (openapi-typescript) and `crates/folo-protocol/src/v1/pyunicode.rs` (Python 3.13's Unicode facts for the v1 port, from `scripts/pyunicode.py`).
- **Clampers (clamp, never reject):** `source/folo_source/contracts/clamp.py` and its port `crates/folo-protocol/src/clamp.rs` agree on `tests/corpus/clamp_v2.json`. `crates/folo-protocol/src/v1/` ports `sanitize_config` with Python's quirks (`str()` of junk values, rounding, truthiness) and matches `tests/corpus/sanitize_config.json`.
- **Checks:** `tests/test_contract_drift.py` compares the spec tables with the schemas and `legacy/v1.py`'s limits, and the effect catalog (`x-folo-effects`) with the engine's registry; `scripts/contracts.py check` validates everything and regenerates into a temporary folder to catch drift; `scripts/conformance.py` drives a running API with schemathesis (`scripts/schemathesis.toml` lists the client API's documented answers it counts as conforming) and checks message files, with one fixture per AsyncAPI message in `specs/fixtures/`.

### Built-in packs (`packs/`, RB-09)
`packs/folo/{core,genres,effects}` and `packs/aliases.json` hold the studio's built-ins as data ([ADR-0038](docs/decisions/ADR-0038-content-library.md) D17 to D20). **Generated, committed, never edited:** `scripts/build_packs.py generate` writes them from `scripts/export_builtins.mjs --trace`, which runs the builders of `web/studio.js` in Node, records each segment's fractions and checks them at every length of D20 against `snap: nearest`. A scene is one `strip.main` role with a `relative`, `snap: nearest` layer per segment. `tests/test_builtin_packs.py` checks the packs' shape, the builtin golden frames byte for byte at 30, 61 and 144 LEDs, and (with Node) the layers at every length through `sanitize_config()` and the migration's conversion. `tests/test_clean_room_gate.py` checks the effect specifications' table. Nothing in the running studio reads the packs yet.

### Node (`crates/`, RB-06, RB-07, RB-08)
The rebuild's Rust node, binary `folo`. It runs the controller's output (RB-08), but no provider kind is registered until RB-10, so it drives no lights yet; the studio still does. Its folders, launches, variables and flags, command line, log and first run follow [ADR-0034](docs/decisions/ADR-0034-the-node-process.md) and the [control plane](docs/architecture/control-plane.md); its client API, live channel and UI files follow [ADR-0035](docs/decisions/ADR-0035-serving-the-client-api.md) and the [client API](docs/specs/client-api.md).
- **Crates:** `folo-settings` (the layering of `node.toml`, `FOLO_*` and flags; the folders), `folo-store` (the documents: clamped with `folo-protocol`, debounced atomic writes, `secrets.json` owner-only: mode 0600, or on Windows a protected access list whose one entry is the owner's), `folo-identity` (the X25519 key and node id), `folo-supervisor` (the engine's process and the node's end of the local link, written against a socket trait) and `folo-node` (the CLI, the run, the axum server, the logger, the home lock and the import). `folo-protocol`'s `catalog` builds the effect catalog from the bundled show schema; its `frames` reads and writes surface and signal frames. The controller's crates are in [Controller](#controller-crates-rb-08).
- **Port:** loopback only while remote access is off. `server.rs` routes every path of `specs/openapi.yaml` behind the [request checks](docs/specs/client-api.md#request-checks); `api.rs` has the resources (documents through `Store::update`, so `If-Match` and item edits are atomic; requests to the engine through `Supervisor::request`; `501` with the run book for the rest; test patterns, discovery and administration through `output.rs`); `defaults.rs` the default surfaces, roles and mappings of a zone created by `POST /zones` ([defaults](docs/architecture/spatial-model.md#defaults): `canvas.room`, `canvas.screen` for a screen zone with a TV, `strip.<target>` per line target and one mapping per target, for what the body leaves out; never on `PUT`, load or migration); `zones.rs` zone activation (RB-08 step 5, ADR-0036 D3 to D6: the effective state is the stored `active` minus the losers of the shared-target check, run on every load and write; conflicts and `take_over` for `POST /zones/{id}/activate` and `/deactivate`, which write `active` through `Store::update`; the engine's `config.apply` gets a copy of the zones with the effective flags, and `folo-supervisor`'s `local_config` skips inactive zones), and `busy` for a test pattern's `409 target_busy`); `output.rs` the controller's output (below); `live.rs` the live channel (a `Hub` of what connections watch, which the engine's `config.apply` follows; zones' `active` is the effective state, with the output's `ambient` and `quiet_s`; providers' states from their senders; `target:<id>` frames from the output); `ui.rs` the UI's files, which `build.rs` embeds from `ui/dist` at build time (without it, `/` says the build has no UI; cargo notices `ui/dist` appearing).
- **Engine data:** the supervisor keeps the newest of what the engine sent (`Received`: notifications by method, `levels` by surface, frames by index), each stamped with an update count so live connections send only what changed. Its `arrivals` counter moves on each surface frame and each link made or ended, and `links` on each link made or ended. From the moment the surfaces move until the engine answers the newest `config.apply`, it takes no surface frame (ADR-0036 D6).
- **Output** (`output.rs`, RB-08): the node is given a provider `Registry` (`node::run_with_providers`; `run` gives an empty one until RB-10). The output task steps `folo-controller`'s `OutputCore` on each frame arrival and at least every 1/60 s: it applies `zones.json` (with the effective activation) and `providers.json` when either changed, takes the engine's new frames (decoded with `folo-protocol`'s `frames`; LZ4 is dropped with a log line), `activity` and `media`, steps the core, and submits each instance's `ElementFrame` to its sender (an empty one when it drives nothing, which releases its targets). Instances are read with `folo_provider::instance_of`, which reads `settings` as the instance's `kind` (the generated union is untagged). One sender runs per enabled `element` instance of a registered kind. A replaced, disabled or removed instance's sender stops on a task of its own (`retiring`, by instance), and a new sender of that instance starts (`start_ready`, each step, never waiting) only once those have finished. When the engine's link is made or ends, the compositor forgets what it sent. Stopping the node stops the output task, then stops every sender at once (each releases its targets and disconnects) while the engine stops. `test` checks `zones::busy` and starts a pattern, and a pattern ends when another active zone comes to drive its target; `provider` serves administration (409 `provider_disabled` for a disabled instance and in the source role, where nothing runs; `AdminError::BadRequest` is 400 `bad_body`). Discovery gives each kind 10 s (`DISCOVER_TIMEOUT`).
- **Capture agent (RB-14, [ADR-0037](docs/decisions/ADR-0037-display-capture-agent.md)):** `folo-supervisor`'s `capture` module is the agent's supervisor, written like the engine's: `CaptureSupervisor` starts `folo-capture` (beside the node or on the `PATH`; `CaptureCommand`) with `FOLO_CAPTURE_LINK` and `FOLO_CAPTURE_TOKEN` in its environment while capture is wanted, restarts it with the engine's backoff, treats 5 s of silence as hung, stops it with `capture.stop` and `restart {component: capture}` and kills it after 5 s. `config::capture_request` decides what is wanted from the documents: the first enabled `inputs.screens[]` entry of the local source and an enabled `screen` layer in the active scene of one of its surfaces (the controller role with the local engine running), with the box of D5. `GET /api/v1/capture-link` (`server.rs`) serves the node's end: loopback only (`403 loopback_only` otherwise, even with remote access on), subprotocol `folo.capture.v1`, a JSON handshake within 5 s, the token compared in constant time before the one-agent test, messages up to 400,000 bytes. Each `0x04` frame goes unchanged into `ScreenFrames`, a newest-wins slot that the engine's link session sends from (frames from before the engine linked are not sent). The node never reads, logs or keeps a frame. `capture.status` is clamped and its `restore_token` goes to `CaptureLog`, which writes `capture.restore_token` in `secrets.json`; `SourceLive.screen` (`CaptureSupervisor::screen_live`, on the `input:local` topic) never has it. `--service` launches start no agent: the state is `unavailable` and the screen list empty. `inputs.list` and `GET /sources/{id}/inputs` get their `screens` from the linked agent, or from `folo-capture screens --json` (`api::with_screens`). `folo status` shows the worker `folo-capture` beside `folo-source`.
- **Tests:** `crates/folo-node/tests/capture.rs` runs the node with a fake engine that records the screen frames it gets and a fake agent (both this test binary run again, writing to a folder named after the node's port outside the home): wanted and not wanted, the kill after 5 s, a hung agent, the auth paths (bad token, silent connection, a second agent, an oversize message), the restore token's round trip and stripping, `screens`, `--service`, and the privacy test (marker pixels in the frames are in no log, status, answer, live message or file of the home). `crates/folo-node/tests/node.rs` runs the node in-process with a fake engine, which is the test binary run again by the node (the `fake_engine_*` tests act as an engine only when the link's variables are set). `cli.rs` runs the built `folo` for its exit statuses and `folo status` without a node. `stops.rs` stops it with SIGTERM and SIGINT, or on Windows with Ctrl+C and Ctrl+Break, sent by the test binary run again as a helper that attaches to the node's console (the node takes Ctrl+C back when its parent had it off, as the engine does), and runs a managed launch with the user's folders moved into a test folder. The answers of `/system/status` and `/system/reload` are checked against the generated `NodeStatus` and `ReloadResult`. `api.rs` runs the API and the live channel against a fake engine that streams what `config.apply` asks for, checks answers against the generated types, and keeps the live messages it got in `target/tmp/live-messages`, which CI checks against AsyncAPI. Its output tests register `folo-provider`'s `RecordingKind`: the engine's frames reach the recording provider, the target and provider show on the live channel, deactivating a zone and stopping the node release the target; test patterns and `target_busy`; discovery and administration (501, 404, 409 and 502). `server.rs`'s unit tests send requests from other addresses (`MockConnectInfo`) for the client-address and key checks. `real_engine.rs` needs `FOLO_E2E=1`; CI's Rust jobs set it up with uv. It also checks the live channel streams the real engine's spectrum, surface, levels, input and beat.

### Capture (`crates/folo-capture-core`, `crates/folo-capture`, RB-14)
The display capture agent of [ADR-0037](docs/decisions/ADR-0037-display-capture-agent.md) and [display capture](docs/integrations/display-capture.md). The core, the link client and the node's side exist: no Windows or Linux backend and no black-bar detector yet, so the agent captures nothing real; only the synthetic and replay backends run end to end.
- **`folo-capture-core` (safe code only):** `types` (screens, `CaptureRequest`, `CaptureStatus`, flags, bars, `Picture`), the `CaptureBackend` trait (list screens, start, next picture, stop) and `error`; `codec` (the `FOLC` frame: 36-byte header and rgb8, bounds-checked, 400,000-byte message cap); `color` (linear-light averaging, `downscale`, the reference the shaders are tested against) and `sizing` (fit in a box, the box the node asks for); `pacer` (`FramePacer`: sequence numbers, the rate limit, the repeat after 0.25 s and `held`); `synthetic` and `replay` (the test backends; the replay file format is in display capture). Time is passed in, and `clock::ProcessClock` stands in for the operating system's counter until the backend crates bring theirs.
- **`folo-capture` (binary):** `folo-capture [--backend auto|dda|wgc|gdi|portal|synthetic|replay:<file>] [run | screens [--json]]`. `screens --json` prints `{"screens": [...]}` and works for `synthetic` and `replay:<file>`; the other backends answer "isn't built yet" with exit status 1. `run` needs `FOLO_CAPTURE_LINK` and `FOLO_CAPTURE_TOKEN` (exit 2 without them) and dials the node. No command means `run`.
- **The link client (`link.rs`, `worker.rs`, `logs.rs`):** `worker::Agent` is the capture side as a state machine (`off` until `capture.start`, `starting`, `capturing`, `paused` on `held` pictures, `error`; it answers `screens.list`, `capture.start` (in place for the same screen, with `-32602` and `-32000` as the link says), `capture.stop`, and `-32601` for the rest), `worker::spawn` runs it on a thread because backends block, and the frames and status reach the link through newest-wins `watch` channels. `link::run` dials with the subprotocol `folo.capture.v1`, sends the hello `{version: ["1"], agent, token}`, answers requests (`restart {component: capture}` stops the capture, answers `{}` and ends the agent with 0; other components get `-32602`), sends `capture.status` at once, on a change and at least once a second, sends frames newest wins with a 5 s send timeout, stops capturing when the link is lost, reconnects with backoff 1 to 60 s cut by up to 25%, and exits with 1 after 30 s without its node or when the node rejects it. `Timing` holds those times so tests can shorten them. Logs are JSON lines on stderr in the engine's format, which the node writes into its own log; they never hold pixels or the token. The frame clock is `ProcessClock` until the backend crates bring the operating system's.
- **Tests:** `crates/folo-capture-core/tests/` (the codec against `specs/fixtures/screenFrame--rgb8.folc`, the colour and sizing values, the pacer, the backends, the JSON shapes against the generated types) `crates/folo-capture/tests/cli.rs` (exit statuses and `screens --json`), `tests/link.rs` (the link client against a fake node: handshake, error codes, status, frames, newest wins, reconnecting, giving up, the oversize message) and `tests/binary.rs` (the built agent on the synthetic backend against the same fake node). `cargo test -p folo-capture-core -p folo-capture`.

## Changes from upstream (so far)

- **The upstream effect runner** (`main.py`, `effects/`, `lib/`): ported to Python 3 and Windows (`soundcard` loopback with numpy instead of GStreamer, the layout from Hyperion.NG, palettes and a Deep Night color set), then removed on `v1` ([ADR-0023](docs/decisions/ADR-0023-legacy-code-and-licensing.md)). The port stays on `main` and in the `v0.9.0-legacy` tag.
- **FOLO studio** (formerly Audio Studio): the `studio.py` server, engine, config store and web UI.
  - **Editing:** spectrum and strip editors, inspector, live meters and a perceptual brightness bar.
  - **Triggers:** Level, Hits and Beat.
  - **Libraries:** reactive presets, 14 whole-strip effects, and 14 genre presets with tags, all in a left-hand effects menu grouped under umbrella categories with search.
  - **Your presets:** save, tag, edit and delete them, and a ★ default restored by Reset all.
  - **Colors by range:** bulk recoloring per spectrum range.
  - **Live sync:** pages follow changes made elsewhere, LED count changes are followed with segment scaling, and Undo is offered for whole-strip changes.
  - **Ambient light:** audio reactive On / Auto / Off, with a solid or breathing ambient color when the music stops.
  - **Look and speed:** glass panels over a Deep Night glow. The page draws only when new data or input arrives, so it uses about a quarter of the CPU it used to.
  - **The engine as a package:** `folo-source` (`source/folo_source/`), with JSON-line logs and a clean stop on signals and console events.
  - **A lighter engine:** each audio packet is analysed in about half the time (the same output, byte for byte), quiet windows skip the FFTs, Hyperion only gets frames that changed, and hidden pages stop streaming.
  - **Audio input:** the studio's own WASAPI capture. It listens to an output, an input, only one app or everything except one app, follows default output switches and app restarts, and has input meters, stats, gain and auto level.
  - **LedFx effects:** six moving modes (scroll, drops, scan, wipe, flow, fire) with per-segment speed and size, and an LedFx group of 18 effects (Energy, Scroll, Wavelength, Power, Rain, Spotlight, Scan, Scan multi, Strobe, Blade Power+, Bar, Multicolor bar, Bands, Equalizer, Blocks, Fire, Lava lamp, Marching).
  - **Beat tracking:** the studio finds the tempo, the beats and the bar itself. A Beat trigger flashes segments every half beat, beat, two beats or bar, with a timing offset, five "On the beat" reactive presets and the Beat sync and Half-time pulse effects.
  - **Now playing:** the song, artist and album art from Windows' media sessions, player controls, album colors on the LEDs, and the ambient light as soon as the music is paused.

## Conventions

- **Testing:** never test on a live config or real LEDs. Use the port-8778 copy with output off (see Commands), and afterwards confirm the live `studio.json` is untouched.
- **Colors:** the built-in looks use the **Deep Night** palette: `#0033CC` deep blue, `#000080` navy, `#4B00FF` UV, `#FF3399` pink, `#FF00FF` magenta, `#800080` purple, `#8F00FF` violet, `#4B0082` indigo. Lows are blue (`LOWS_PAL`), mids pink (`MIDS_PAL`) and highs violet (`HIGHS_PAL`).
- **Outside the repo:** say what you'll change before touching Hyperion settings, WLED state or anything else outside this folder. Hyperion's settings DB (`%USERPROFILE%\.hyperion\db\hyperion.db`) may only be edited while Hyperion is stopped, after a backup.
- **Docs:** `docs/` is the source of truth (see the top of this file). This file is the single agent guide to the current code (CLAUDE.md imports it). Update it, the README's FOLO studio section and the affected documents in `docs/` whenever features change.
- **Machine-specific notes:** a particular setup's hardware, network, live state and owner preferences go in `AGENTS.local.md` (git-ignored, imported for Claude Code by `CLAUDE.local.md`). Read it when it exists.
- **Files:** UTF-8 with LF line endings, except CRLF for `.bat` and `.cmd` (`.gitattributes`, `.editorconfig`). Four-space indents, two for JS, CSS, HTML, JSON, YAML and Markdown.
- **Ignored files:** `studio*.json`, their `.v1.bak` copies and `data/` at the root, build outputs and the local agent notes. Other JSON files are tracked, so keep personal copies of configs outside the repository.

## Troubleshooting

- **Dark strip with Hyperion and the studio both fine:** check for audio first. Segments with no idle glow (`floor` 0) render black in silence, and the stream's `spectrum` sits at −120 when nothing plays.
- **Hyperion's own audio effect or a pinned source hides the studio:** these API commands work without auth: `{"command":"sourceselect","auto":true}` and `{"command":"componentstate","componentstate":{"component":"AUDIO","state":false}}`. `serverinfo` → `priorities` shows which source is visible.
- **WLED goes dark when Hyperion stops:** with "stay on after streaming" off, re-enable it with `POST http://<wled-host>/json/state` and `{"seg":[{"id":0,"on":true}]}`.
- **Stopping the studio:** Ctrl+C in its console stops it cleanly. Otherwise it runs as two `python.exe` processes whose command line contains `studio.py` (the venv launcher plus the interpreter); stop both.

## Not tested yet

- Linux capture and MPRIS on a real desktop: the tests ran against PulseAudio with null sinks and a fake MPRIS player on a private bus, not PipeWire's `pipewire-pulse`, a real device or real players. Album art at web (`https://`) URLs isn't fetched.
- A real default output switch or an unplugged device. The notification and reopen paths were only tested with simulated changes, to leave the owner's audio alone.
- The glitch counter: it was never seen above 0.
- Switching audio devices while the studio runs.
- The beat tracker on real music: it was only measured on synthetic songs. The timing offset that suits the real LEDs isn't known yet.
- The player controls (previous, play/pause, next): never pressed, to leave the owner's player alone. Only refusing a bad action was tested.
- Players other than Arc, and album art from a music app (Arc gave a video thumbnail).
- The controller's output on real devices: no provider kind is registered yet (RB-10), the flash limiter on real lights, Windows timer granularity against the 1/60 s step, and the 0.8 ms budget on the owner's hardware.
- The node's client API from another machine, and with a real UI build: remote access, the `401` for other machines and the Origin check were only tested in-process, and the UI's files only with a throwaway `ui/dist` (the UI arrives with RB-11).
