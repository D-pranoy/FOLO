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
| Syntax check Python | `.venv\Scripts\python.exe -m py_compile studio.py app\studio_config.py app\studio_engine.py app\audio_input.py app\wasapi.py app\beat_tracker.py app\now_playing.py app\json_client.py main.py` |
| Syntax check the UI | `node --check web\studio.js` |
| Ask Hyperion for its LED layout | JSON API: send `{"command":"serverinfo","tan":1}\n` to `localhost:19444` (see `app.json_client.fetch_server_info`) |

**Restart rules:** Python changes need the studio process restarted (kill the `python.exe` running `studio.py`, then start `studio.bat` again; LEDs blink briefly). `web/*` files are served with `no-store`, so UI changes only need a page refresh.

**Testing without touching the live setup:** `studio.json` is the live config, edited through the UI, and the studio drives real LEDs. Test destructive or experimental changes on a second instance with a copied config and Hyperion output disabled:

```
# copy studio.json elsewhere, set "hyperion": {"enabled": false, ...} in the copy, then
.venv\Scripts\python.exe studio.py --port 8778 --no-browser --config <copy>.json
```

Engine logic can be tested offline by importing `app.studio_engine.Analyzer` / `Renderer` and feeding numpy blocks, frames × channels or mono (see how `Renderer.render(config, spectrum, dt)` is called from `StudioEngine._on_audio`). For A/B work on real audio, record the capture device to a `.npy` (`device.recorder(samplerate=48000).record(numframes=...)`) and feed both engines the same 480-frame packets so their frames line up. Stacking the frames of one segment into an image (time down, LEDs across) is a quick way to see whether a moving mode looks right. A speed-up should leave the output byte for byte the same: load the committed `studio_engine.py` (`git show HEAD:app/studio_engine.py`) with `importlib`, run both on the same packets with the renderers' `rng` seeded alike, and compare frames, levels and spectra, including silence, gains up to +54 dB and every mode. Browsers throttle timers in hidden tabs to about 1 s, so in-page JS checks need multi-second waits.

**Testing app capture without sound:**
- **Tone:** play an inaudible tone (18.5 kHz at −70 dBFS) from a process with a distinctive name. A copy of the base interpreter renamed `tonegen.exe` works, run with `PYTHONHOME` set to the base install and `PYTHONPATH` to the venv's site-packages.
- **Check:** "only" that app must show the tone, and "except" it must show silence (−120 dB).
- **Time:** `time.process_time()` ticks in 15.6 ms steps on Windows, so CPU comparisons need long or repeated runs.

**Testing the beat without sound:**
- **Offline:** synthesize songs with known beat times (kick, snare, hats, a bass line and a chord per bar, at 60–180 BPM, four-on-the-floor, rock, swung hip-hop, tempo drift, breakdowns), feed `Analyzer` and `BeatTracker` the same 480-frame packets, and score the fired beats against the true ones (±70 ms) plus the bar starts. Also feed music without a steady beat (pads, random piano notes, speech-like bursts): it must never lock.
- **In the page:** run `studio.py` with `audio_input.AudioInput` replaced by a fake that streams synthetic songs at real-time pace (no sound plays), then check the tempo pill, the Music card and beat segments.
- **Player controls:** don't press play/pause, next or previous while testing: they act on the owner's real player.

`.claude/launch.json` (git-ignored) has a `studio-test` configuration for the in-app browser preview: port 8778 with a config copy in a session scratchpad. Point `--config` at your own copy before using it.

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

### Config (`app/studio_config.py`)
`sanitize_config()` is the single source of truth for the schema and its limits. Everything the UI sends goes through it.

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

### Engine (`app/studio_engine.py`)
- **Audio input (`app/audio_input.py` on `app/wasapi.py`):** the studio's own WASAPI capture through cffi. soundcard is only used by the legacy effects.
  - **Sources** (config `audio`):
    - an output's loopback (`device`: `default` or a name)
    - an input (`in:` + name)
    - only one app, or everything except one app (`app`: exe name, `app_mode`: `only` / `except`). This uses Windows 10 2004+ process loopback (`ActivateAudioInterfaceAsync` with a cffi COM completion handler) on the app's top process (`app_root()`), so its helper processes count too, whatever output it plays on.
  - **Stream:** event driven (`AUDCLNT_STREAMFLAGS_EVENTCALLBACK`, falling back to polling) in the MMCSS "Pro Audio" class. Every wake drains all queued packets (`GetBuffer` until `AUDCLNT_S_BUFFER_EMPTY`), so the analysis always runs on the newest audio, at 48 kHz float. App capture takes only the flags in Microsoft's sample: LOOPBACK, EVENTCALLBACK and AUTOCONVERTPCM.
  - **Silence:** an output's loopback sends no packets during silence, while app capture sends silent ones. After a gap of more than `GAP` (0.1 s) the batch is `fresh` and the analyzer window is cleared.
  - **Following:** `default` follows Windows' default output through `IMMNotificationClient` notifications (`wasapi.device_changes()`), plus a 10 s safety poll, because asking for the default costs 2 ms. An app is re-attached when its top process changes, waits while it isn't running (state `waiting`), and "except" listens to everything until the app starts. A lost device (`AUDCLNT_E_DEVICE_INVALIDATED`) is reopened after 0.5 s.
  - **Stats** (`AudioInput.status()`): packets/s, the median delay from WASAPI's QPC stamp to the engine (about 2 ms for loopback, about 10 ms for app capture, which stamps the packet's start), discontinuities (`glitches`), and a falling peak meter of the first two channels with a clip flag. The per-channel peaks are computed once and passed to `Analyzer.push()` for its active-channel detection.
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
- **`Renderer`, per config:** `_prepare()` works out what only changes with the config (band plans of every segment, visibility, layouts, static gradients, the background) when `render()` gets a config object it hasn't seen. This relies on the store replacing the config, never mutating it. Every segment's level then comes from one `reduceat` per window, and single values are shaped in plain Python (`shape_one()`, `color_at()`), because numpy costs microseconds per call even on one number.
- **`Renderer`, per segment:**
  1. Band level, plus a running average (`AVERAGE_MS`) used by hits.
  2. Shaping: threshold maps to 0, ceiling to 1, then gain, then the `curve` exponent. Beat segments (`_beat_target()`) shape the band's level held at its peaks (falling `BEAT_HOLD_FALL`, 12 dB/s), and only for a moment after each beat of their grid (`BeatTracker.fired(beats)`): at least `BEAT_PULSE` (40 ms, so a frame Hyperion gets shows it) or three times the attack. Else their target is 0.
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
  - **Hits** for drops and wipe: the shaped target rising past `HIT_ON` (0.5) after dropping below `HIT_OFF` (0.25), at most one per `HIT_GAP`. On the beat trigger every loud enough beat is a hit.
  - **Idle glow** (`floor`) is the minimum brightness of what the mode draws (scan eye, wipe colors, flow, flame height); scroll and drops use it as a dim palette backdrop.
- **Ambient light (`Ambient`, in `Renderer.render` before `master`):**
  - **Blend:** crossfades the segments' frame with `ambient.color × level`; `breathe` pulses between 30% and 100% over `BREATHE_PERIOD` (8 s).
  - **Silence:** the audio counts as silent while its loudest bin is below `QUIET_DB` (−70 dB), and `quiet` counts the seconds.
  - **Fades:** into the ambient light over `AMBIENT_FADE_IN` (1.5 s), eased; back to reactive in `AMBIENT_FADE_OUT` (0.15 s), so the first beat after a pause still shows.
  - **Paused player:** with `now_playing.pause_ambient`, Auto switches after `PAUSE_QUIET` (0.3 s) of silence instead of `idle_after` while the player says it's paused. Other sound (a game) keeps it reactive.
- **Beat tracker (`app/beat_tracker.py`, `BeatTracker`):** pushed with every spectrum right before it's rendered (`StudioEngine._track_beat()`), silence included.
  - **Onsets:** 100 per second: how much 24 log-spaced bands (40 Hz–10 kHz, their loudest bin, floored 50 dB below the loudest band) rose over 20 ms, averaged.
  - **Tempo:** every 0.1 s, the autocorrelation of the last 8 s of onsets (a 1 s moving average taken out, older onsets weighted down over 3 s) at each candidate period from 60 to 200 BPM, plus its double and quadruple, weighted by a log-Gaussian around 120 BPM and towards the tempo already found. A far tempo must persist 1 s before it's taken.
  - **Lock:** `confidence` is the autocorrelation at the beat, smoothed. Beats fire only after it stays above `LOCK_ON` (0.3) for 0.5 s, and stop below `LOCK_OFF` (0.2) or after 1 s of silence; 3 s of silence forgets the tempo. Synthetic music with a steady beat measured 0.35–0.8. Pads, random piano notes and speech-like bursts stayed below 0.3, apart from peaks (up to 0.32) shorter than the half second a lock needs.
  - **Clock:** `pos` counts beats and only moves forward. It runs at the tempo plus a correction (at most ±25%) that makes up the phase error over 0.8 s, so no beat fires twice or is skipped. The phase comes from a comb of beats over the last 4 s of onsets. Beats fire `lead` early: `ONSET_DELAY` (25 ms, how far the onsets lag the sound) plus `beat.offset`.
  - **Bars:** each beat of four collects accents: bass onsets (below 150 Hz) and how much the bands below 4 kHz changed since the beat before (chords and bass notes change on the first beat, drums repeat). The beat with the most (15% margin) is `downbeat`.
  - **When it runs:** only while an enabled segment uses the beat or a page streamed in the last 3 s (`_beat_wanted()`); otherwise `skip()`. A new song from now playing resets it.
  - **Measured:** on synthetic songs it locks after about 4 s and every beat lands within 10 ms. It costs about 50–70 µs a packet (0.5–0.7% of a core) while it runs.
- **Now playing (`app/now_playing.py`, `NowPlaying` thread):** Windows' media sessions (`GlobalSystemMediaTransportControlsSessionManager`) through the `winrt` packages; without them its state is `unavailable`.
  - **Session:** a playing one first, and among those the app the studio captures (`audio.app` in only mode, matched in the app id: Arc reports `Arc.exe`).
  - **Following:** the sessions' change events wake the thread, plus a check every second, and the title is read again every 5 s anyway.
  - **Album art:** read when the song changes (again after 2 s if the player hasn't set it yet), decoded to 48×48 by `BitmapDecoder`. `cover_colors()` picks up to five hues from a histogram weighted by chroma², at full brightness and a bit more saturated; grey or dark art gives none.
  - **Album colors on the LEDs:** `Renderer._prepare(config, colors)` swaps each segment's palette for as many art colors (`cover_palette()`: the strongest first for lows, the second for mids, the third for highs), so a new cover is worked out once.
  - **Controls:** `control()` runs `try_toggle_play_pause_async` / `try_skip_next_async` / `try_skip_previous_async` on the shown session.
- **Idle thread:** after `IDLE_AFTER` (0.1 s) without an audio packet, it renders silence at `IDLE_FPS` (30), so fades, releases and the ambient light keep going. Packets stop when nothing plays on some devices, or when capture fails. Rendering from both threads is serialised by `_render_lock`. `fps` counts audio packets only, so it reads 0 while idle.
- **`HyperionSender` thread:** sends JSON `color` commands with priority 100, origin `Audio Effects` and `duration` 2000 ms, so Hyperion releases the LEDs if the studio dies. Hyperion rejects an `origin` longer than 20 characters.
  - **Rate:** it checks for a new frame at the configured fps and sends it when it changed. An unchanged frame (silence, a solid ambient light) goes out again only every `KEEPALIVE` (0.5 s), well within `duration` and WLED's 2.5 s live timeout. Parsing our 40 commands a second cost Hyperion's JSON thread about 6% of a core.
- **Stream snapshot fields:** `spectrum` (192 log-spaced bands, 20 Hz–20 kHz, as dB integers), `leds` (hex), `levels {id: [raw dB, value 0–1, average dB]}`, `led_count`, `revision`, `server_id` and `status`. The bands and the rounded levels (`shown_levels()`; `render()` returns them unrounded) are worked out once per streamed frame and shared by every open page. `status.ambient` is the fade (0 reactive … 1 ambient light) and `status.quiet` the seconds of silence. `status.input` is `AudioInput.status()` plus `gain` and `auto` (the auto level gain), and `status.audio.device` is the source's label. `status.beat` has `bpm` (0 until found), `confidence`, `locked`, `phase` (0–1 through the beat) and `bar_beat` (0 is the first). `status.media` is `NowPlaying.status()`: `state` (`off`, `unavailable`, `error`, `none`, `playing`, `paused` or `stopped`), `app`, `title`, `artist`, `album`, `art` (an id, 0 without art) and `colors`.

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

### Legacy effects (`main.py`, `effects/`, `app/hyperion.py`, `app/gui.py`)
- **`main.py`:** reads the LED layout from Hyperion (or a classic config file), fakes the Hyperion effect API in `app/hyperion.py`, runs `effects/<name>.py` through `runpy`, and streams over JSON or protobuf. A single-column strip layout is split in half into left and right.
- **`effects/spectrum_dump.py`:** emulates GStreamer's `spectrum` and `level` elements with numpy over `soundcard`, keeping the original effect code unchanged.
- **`effects/palette.py`:** the color-list support used by both effects.

## Changes from upstream (so far)

- **Python 3 / Windows port:** GStreamer replaced by `soundcard` loopback plus numpy emulation. The layout is read from Hyperion.NG instead of `hyperion.config.json`, Hyperion.NG JSON is accepted (short origin, clear by priority), and there are `run.bat` and `--list-devices`.
- **Legacy effects:** palette support and a Deep Night color set (deep blue, navy, UV, pink, magenta, purple, violet, indigo).
- **FOLO studio** (formerly Audio Studio): the `studio.py` server, engine, config store and web UI.
  - **Editing:** spectrum and strip editors, inspector, live meters and a perceptual brightness bar.
  - **Triggers:** Level, Hits and Beat.
  - **Libraries:** reactive presets, 12 whole-strip effects, and 14 genre presets with tags, all in a left-hand effects menu grouped under umbrella categories with search.
  - **Your presets:** save, tag, edit and delete them, and a ★ default restored by Reset all.
  - **Colors by range:** bulk recoloring per spectrum range.
  - **Live sync:** pages follow changes made elsewhere, LED count changes are followed with segment scaling, and Undo is offered for whole-strip changes.
  - **Ambient light:** audio reactive On / Auto / Off, with a solid or breathing ambient color when the music stops.
  - **Look and speed:** glass panels over a Deep Night glow. The page draws only when new data or input arrives, so it uses about a quarter of the CPU it used to.
  - **A lighter engine:** each audio packet is analysed in about half the time (the same output, byte for byte), quiet windows skip the FFTs, Hyperion only gets frames that changed, and hidden pages stop streaming.
  - **Audio input:** the studio's own WASAPI capture. It listens to an output, an input, only one app or everything except one app, follows default output switches and app restarts, and has input meters, stats, gain and auto level.
  - **LedFx effects:** six moving modes (scroll, drops, scan, wipe, flow, fire) with per-segment speed and size, and an LedFx group of 18 effects (Energy, Scroll, Wavelength, Power, Rain, Spotlight, Scan, Scan multi, Strobe, Blade Power+, Bar, Multicolor bar, Bands, Equalizer, Blocks, Fire, Lava lamp, Marching).
  - **Beat tracking:** the studio finds the tempo, the beats and the bar itself. A Beat trigger flashes segments every half beat, beat, two beats or bar, with a timing offset, five "On the beat" reactive presets and the Beat sync and Half-time pulse effects.
  - **Now playing:** the song, artist and album art from Windows' media sessions, player controls, album colors on the LEDs, and the ambient light as soon as the music is paused.

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

- A real default output switch or an unplugged device. The notification and reopen paths were only tested with simulated changes, to leave the owner's audio alone.
- The glitch counter: it was never seen above 0.
- Studio shutdown with Ctrl+C.
- Switching audio devices while the studio runs.
- Legacy `main.py --gui` and protobuf/matrix mode.
- The beat tracker on real music: it was only measured on synthetic songs. The timing offset that suits the real LEDs isn't known yet.
- The player controls (previous, play/pause, next): never pressed, to leave the owner's player alone. Only refusing a bad action was tested.
- Players other than Arc, and album art from a music app (Arc gave a video thumbnail).
