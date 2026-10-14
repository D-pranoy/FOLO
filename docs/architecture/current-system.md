# The current system (before the rebuild)

Status: Accepted (describes the code on `main`, the frozen legacy studio, at the start of the rebuild, including the audio input, beat tracking, now playing, ambient light and glass UI additions)

This is the baseline that the rebuild replaces. Its behaviour is preserved where the [UI parity checklist](../specs/ui-parity-checklist.md), the [parity mode](pipeline.md#parity-mode) and [ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md) require it. `AGENTS.md` remains the detailed guide to running it until it is replaced.

## Shape

| Part | Files | Notes |
|---|---|---|
| Studio server | `studio.py` | stdlib `ThreadingHTTPServer`, bound to `127.0.0.1:8777`, refusing non-local `Host` headers and non-JSON POSTs ([endpoints](#studio-server-endpoints)). Polls Hyperion's LED count every 5 s. |
| Config | `app/studio_config.py` | `sanitize_config()` clamps every value and never rejects. `ConfigStore` replaces the config dict wholesale, debounces saves to `studio.json`, and keeps `revision` and `server_id`. |
| Audio input | `app/audio_input.py` on `app/wasapi.py` | FOLO's own WASAPI capture through cffi ([below](#audio-input)) |
| Engine | `app/studio_engine.py` | Analyzer, auto level, renderer, ambient light, idle thread, Hyperion sender |
| Beat tracker | `app/beat_tracker.py` | Tempo, beats, bar ([below](#beat-tracking)) |
| Now playing | `app/now_playing.py` | Windows media sessions through `winrt` ([below](#now-playing)) |
| JSON client | `app/json_client.py` | Hyperion JSON `color` commands with `duration` |
| UI | `web/studio.js` (3,679 lines), `index.html`, `studio.css` (1,003 lines) | Vanilla JS, no framework or build step; works offline; glass visual design |
| Legacy effects | `main.py`, `effects/`, `lib/`, `app/gui.py`, `app/hyperion.py`, `matrix-config.js`, `toggle.sh`, `.asoundrc`, `config.json` | The upstream Python 2 effect runner, ported to Python 3. Not used by the studio; soundcard, commentjson, webcolors and protobuf are used only here. Removed from `v1` in RB-00 ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)); it stays on `main` and in the `v0.9.0-legacy` tag. |

## Studio server endpoints

| Endpoint | Purpose |
|---|---|
| `GET /api/state` | Config, audio devices, apps playing audio, display constants |
| `GET /api/sources` | Devices and apps playing audio; refreshed by the Audio input card every 5 s |
| `GET /api/stream` | Server-sent events at 30 fps |
| `GET /api/art` | Album art; the page adds the art id so a new cover isn't cached |
| `POST /api/config` | The whole config |
| `POST /api/reset` | Reset |
| `POST /api/layout` | Re-read Hyperion's LED count |
| `POST /api/media` | `play_pause`, `next` or `previous` on the player shown as now playing |

## Audio input

- **Sources:**
  - an output's loopback (`default`, which follows Windows' default output, or a named output)
  - an input (`in:<name>`)
  - **only one app**, or **everything except one app**, using Windows 10 2004+ process loopback on the app's top process, so its helper processes count too
- **Stream:**
  - event-driven in the MMCSS "Pro Audio" class, falling back to polling
  - every wake drains all queued packets, so analysis always runs on the newest audio (48 kHz float)
  - after a gap of more than 0.1 s, the analyzer window is cleared
- **Following:**
  - default-output changes arrive through `IMMNotificationClient`, with a 10 s safety poll
  - apps are re-attached when their top process changes; the input waits while the app isn't running
  - lost devices are reopened after 0.5 s
- **Stats:** packets/s, the median delay from the WASAPI timestamp (about 2 ms for loopback, about 10 ms for app capture), glitches, and falling peak meters with a clip flag.

## Engine

- **Gain and auto level:**
  - `audio.gain` (±24 dB), plus an automatic gain when `audio.auto_level` is on, are added to every level.
  - Auto level follows the loudest bin (300 ms rise, 8 s fall) towards −15 dB, clamped to −12…+30 dB.
  - Silence detection always uses the level without gain.
- **Analysis:**
  - Hann-windowed FFTs: 4096 points (`full`: level triggers, the display) and 2048 points (`fast`: hits triggers, computed only when needed).
  - Band level is the loudest bin in the range, interpolated for bands narrower than a bin, using `np.maximum.reduceat` over cached index plans.
  - The power of every channel that has carried sound (above −100 dBFS, sticky) is averaged.
  - `CALIBRATION_DB = −9.54 dB` keeps the scale the presets were tuned on.
  - A doubled history buffer avoids shifting the window on every packet.
  - **Quiet windows** that can't lift any bin above −120 dB return the silent spectrum without computing FFTs.
- **Rendering:**
  - Per configuration object, `_prepare()` precomputes band plans, visibility, layouts, static gradients and the background. This relies on the store replacing the config wholesale.
  - Per segment:
    1. Band level, plus a 1 s running average for hits.
    2. Shaping: threshold → 0, ceiling → 1, then gain, then the curve exponent.
    3. Attack/release smoothing.
    4. The mode, laid out by direction with step indices.
    5. A max-blend into the frame, then `master`.
  - **Beat segments** shape the band level held at its peaks (falling 12 dB/s), only for a moment after each beat of their grid (at least 40 ms, or three times the attack).
- **Moving modes** (scroll, drops, scan, wipe, flow, fire) keep per-segment state. Hits fire when the shaped value rises past 0.5 after falling below 0.25, at most one per 60 ms. On the beat trigger, every loud enough beat is a hit.
- **Ambient light** (before `master`):
  - crossfades the segments with `ambient.color × level`
  - `breathe` pulses between 30% and 100% over 8 s
  - silence means the loudest bin is below −70 dB
  - fades into the ambient light over 1.5 s (eased) and back over 0.15 s
  - with a paused player and `pause_ambient`, Auto switches after 0.3 s of silence
- **Idle thread:** after 0.1 s without audio packets, it renders silence at 30 fps so fades, releases and the ambient light continue.
- **Hyperion sender:**
  - priority 100, origin `Audio Effects`, `duration` 2000 ms
  - sends only frames that changed, plus a 0.5 s keepalive (parsing 40 commands a second cost Hyperion about 6% of a core)

## Beat tracking

- **Onsets:** 100 per second, from the rise of 24 log-spaced bands (40 Hz–10 kHz) over 20 ms.
- **Tempo:** every 0.1 s, the autocorrelation of the last 8 s of onsets, over 60–200 BPM (with double and quadruple periods), weighted towards 120 BPM and towards the current tempo.
- **Lock:** beats fire after confidence stays above 0.3 for 0.5 s. They stop below 0.2 or after 1 s of silence; 3 s of silence forgets the tempo.
- **Clock:** a forward-only beat position with a bounded (±25%) phase correction, so no beat fires twice or is skipped. Beats fire early by the onset delay (25 ms) plus `beat.offset` (±200 ms).
- **Bars:** the downbeat is the beat with the most bass onsets and harmonic change (15% margin).
- **Measured:** on synthetic songs it locks after about 4 s, every beat lands within 10 ms, and it costs about 50–70 µs per packet while running. It runs only while needed, and a new song resets it.

## Now playing

- Windows media sessions (`GlobalSystemMediaTransportControlsSessionManager`) through the `winrt` packages. Without them the state is `unavailable`.
- Prefers a playing session, and among those the app being captured.
- Album art is decoded to 48×48. Up to five hues are picked, weighted by chroma², and can replace segment palettes ("Album colors": strongest for lows, then mids, then highs).
- Player controls: play/pause, next, previous.

## Built-in content

The menu holds 46 built-ins:
- 14 effects, including Beat sync and Half-time pulse
- 18 LedFx-inspired effects, where Bar and Multicolor bar use the beat trigger
- 14 genre presets, which use layouts and refer to reactive presets by name

There are also 25 reactive presets in 5 groups (including "On the beat"), 9 palettes, 6 colour ranges and 8 menu categories.

## UI

- **Visual design:** glass panels over Deep Night glows ([visual design](../specs/visual-design.md)).
- **Panels:**
  - effects menu, spectrum, LED strip and track, segments, colours by range, reactive presets
  - selected segment (inspector)
  - **Ambient light**, **Music**, **Audio input**, Settings
- **Top bar:**
  - status pills (audio, Hyperion, fps)
  - a now-playing chip and a tempo pill with a beat dot
  - the audio-reactive switch (On, Auto, Off) and an ambient indicator
  - brightness and the output toggle
- **Drawing:** only on invalidation (stream messages, input, resize, state changes). Static layers are cached offscreen, and live bars move by `transform`. A hidden page closes its stream. This brought the page from about 2.2 to about 0.6 browser cores.
- **Layout:** three columns (290 px | flexible | 380 px), two at ≤1500 px, one at ≤1100 px, compact at ≤640 px.

## Known defects (to fix in the rebuild)

**Server**
- An unguarded `int(Content-Length)` in `studio.py`.
- `revision += 1` outside the lock in `studio_config.py`.
- File I/O without an explicit encoding.
- No SIGTERM handling; clean shutdown on Ctrl+C only.
- The renderer's RNG has no seed in the studio (tests inject a seeded one since RB-01).

**Beat tracker** (found by the RB-01 fixtures in `tests/beat_fixtures.py`; the quality tests expect them to fail until fixed)
- On a rock pattern (kicks on 1 and 3) the bar position settles two beats off.
- After a breakdown without drums, the bar position comes back one beat off.
- Perfectly clean sustained tones without a beat (two steady sine tones, overlapping synthetic sine chords) lock it: its periodicity measure doesn't depend on level, so tiny regular fluctuations read as a beat. A noise floor 50 dB down, as any recording has, prevented it in tests.

**UI**
- An Undo button stays clickable after its toast disappears.
- The preset edit form keeps a stale reference and can lose changes.
- The brightness readout isn't clamped.
- The inspector's attack/release limits are below the server's.
- A stale "Presets" label in the Reset all tooltip.
- Inconsistent destructive actions: some have Undo, some don't.
- No keyboard path for canvas editing.
- Some controls are double-click or hover only (gain and beat-timing resets, previews).

**Not yet verified** (from `AGENTS.md`):
- a real default-output switch or unplugged device
- the glitch counter
- the beat tracker on real music and the right LED timing offset
- the player controls on a real player
- players other than one browser

## Measured performance

See [pipeline](pipeline.md#performance-budget).

## Licensing note

The repository is MIT (upstream © Juha Rantanen). The six moving modes and 18 effects were ported with reference to LedFx's source, and LedFx is GPL-3.0. They are rewritten clean-room before the first public binary release ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)).
