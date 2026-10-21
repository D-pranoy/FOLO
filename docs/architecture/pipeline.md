# Pipeline

Status: Accepted · Implementation: Partial (the effect engine's core types, inputs on Windows, the beat signal, the audio-visualizer and post-processing effects, surfaces, and the publisher's frames and local link)

`inputs → effects → surfaces ⟶ compositor → projection → adapters → providers`

The first half runs in a **source** (the effect engine and capture agent). The second half runs in the **controller**. Decided in [ADR-0005](../decisions/ADR-0005-pipeline-and-data-model.md).

## Frames and signals

- **Frame:** an array of linear RGB values, shape `(N, 3)` for a strip or `(H, W, 3)` for a canvas, with metadata:
  - pixel format: `rgbf32` inside a process; `rgb8` or `rgb16` on the wire; a video format reserved
  - `rgbf32` values in the effect engine are on today's 0–255 scale. They are rounded toward zero from the renderer's 64-bit values, so truncating them to `rgb8` gives today's bytes ([parity mode](#parity-mode)).
  - timestamp in monotonic microseconds
  - sequence number
- **Signal:** a named scalar or vector at a rate. Examples: band levels, an activity flag, external data.
- **Clock:** monotonic microseconds everywhere. Sources stamp frames, and the controller measures latency per source and per output.

## Inputs (in a source)

| Input | Description |
|---|---|
| Audio | See [Audio inputs](#audio-inputs) below. Captured in about 10 ms packets and analysed with 4096- and 2048-point Hann-windowed FFTs, as today ([current system](current-system.md)). |
| Media session | Now playing: title, artist, album art, cover colours, play state; player controls. Windows media sessions or Linux MPRIS ([ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)). |
| Screen | One selected screen per source in v1, captured on demand by the capture agent ([display capture](../integrations/display-capture.md)) |
| Generator | Time, LFO, noise; for idle and ambient effects |
| Local ingest | Signals or frames pushed by local programs over loopback ([source link](../specs/source-link.md#local-ingest)) |

A source **lists every input available to it**. Each input is configured and enabled on its own. Inputs that are enabled but unused stay idle; display capture starts only when an active layer needs it.

### Audio inputs

| Option | Windows | Linux (v1) |
|---|---|---|
| Everything you hear (the default output, followed when it changes) | WASAPI loopback, event-driven, MMCSS "Pro Audio" | PulseAudio/PipeWire monitor of the default sink |
| One named output | WASAPI loopback | The sink's monitor |
| An input (microphone, line-in) | WASAPI capture | PulseAudio/PipeWire source |
| Only one app / everything except one app | Process loopback on the app's top process (Windows 10 2004+). Waits while the app isn't running; re-attaches when it restarts. | Later (PipeWire stream targets) |

On Linux the engine uses the PulseAudio client library, which PipeWire's PulseAudio server also serves ([ADR-0031](../decisions/ADR-0031-linux-audio-capture.md)).

- **Every wake drains all queued packets**, so analysis runs on the newest audio. After a gap of more than 0.1 s the analysis window is cleared.
- **Stats** published with the input: packets/s, median delay, glitches, falling peak meters with a clip flag.
- **Gain and auto level:**
  - A manual gain (±24 dB) plus an optional automatic gain are added to every level.
  - Auto level follows the loudest bin (300 ms rise, 8 s fall) towards −15 dB, clamped to −12…+30 dB.
  - Silence detection always uses the level without gain.
- **Quiet windows** that can't lift any bin above −120 dB skip the FFTs and return the silent spectrum.

### Beat signal

- The beat tracker runs in the effect engine on every analysed packet while a layer uses the beat or a client is watching ([current system](current-system.md#beat-tracking)).
- It publishes a beat signal: BPM, confidence, locked, phase within the beat, and beat within the bar.
- Audio-visualizer layers with the `beat` trigger flash every ½, 1, 2 or 4 beats, aligned to the bar.
- Beats fire early by the onset delay plus the zone's **beat timing offset** (±200 ms).

## Effects

- An effect declares an id, a version, its inputs, a JSON Schema for its parameters, and minimum capabilities (for example, a minimum element count).
- **Built-in effects:**
  - **Audio visualizers:** the existing modes fill, pulse, spectrum, shift, scroll, drops, scan, wipe, flow and fire. Each layer has the existing segment semantics: band, trigger (level, hits or beat), beats per flash, threshold, ceiling, gain, curve, attack, release, idle glow, direction, speed, size, peak hold and palette.
  - **Album colours:** when the zone enables them, each layer's palette is replaced by colours from the album art (strongest for lows, then mids, then highs), recomputed only when the cover changes.
  - **Screen:** turns a captured frame into a canvas, with colour, saturation, smoothing and video/game profiles.
  - **Post-processing:** brightness and saturation, mirror, blur, trail. These layers change what is below them in the stack, inside their region, instead of drawing a line; their blend mode doesn't apply. Their parameters are in the [configuration model](../specs/config-model.md#entities-summary).
    - `brightness`: moves each channel toward the cell's luma (`0.2126 R + 0.7152 G + 0.0722 B`) or away from it by `saturation` (0 grey, 1 unchanged), multiplies by `brightness` and clips to 0–255.
    - `mirror`: copies the half of the region that `keep` names, reversed, onto the other half. It works along a strip, or along a canvas's columns (its rows when the layer's `axis` is `linear_y`). A middle cell stays as it is.
    - `blur`: each cell becomes the average of the cells up to `radius` away along a strip, or in the square of that radius on a canvas. Past the region's edges, its edge cells repeat.
    - `trail`: an afterglow. Each cell is at least what the layer gave it the frame before, faded by `e^(−dt / fade)`. It starts over when its region changes size.
- **1D effects on canvases** use axis modes: `linear_x`, `linear_y`, `radial` and `angular`. Every visualizer therefore works on 2D surfaces and in any zone.
  - The effect renders a line of `L` cells. Each cell of the layer's rectangle takes the line's cell `min(L − 1, ⌊t × L⌋)`, where `t` (0–1) is the position of the cell's centre along the axis:
    - `linear_x`: across the columns; `L` is the rectangle's width.
    - `linear_y`: down the rows, from row 0; `L` is its height.
    - `radial`: the distance from the rectangle's centre, 1 at its corners; `L` is half its diagonal, rounded.
    - `angular`: the angle around the centre, clockwise from the side of row 0; `L` is its perimeter.
  - On a strip, the line is the layer's region itself.
- Effects run in-process in the effect engine. An out-of-process host is reserved for future plugins.

## Surfaces and layer stacks

- A surface has a kind (strip or canvas), a resolution, a background and a master level.
- Its layers form an ordered stack. The default blend is **max**, which reproduces today's behaviour; other blend modes are available per layer.
- A layer covers a **region** of the surface: a cell range on a strip, a rectangle on a canvas. On a strip of `N` cells:
  - `cells`: `start` to `end`, both included, as today's segments.
  - `relative`: cells `⌊start × N⌋` to `⌈end × N⌉ − 1`, at least one.
  - `rect`: as `relative` from `x` to `x + width`.
  - On a `W × H` canvas, a `rect` covers columns `⌊x × W⌋` to `⌈(x + width) × W⌉ − 1` and rows likewise, at least one of each. `cells` and `relative` regions select columns, over every row.
  - A region is clipped to the surface. A layer whose region is empty draws nothing.
- **Rendering a stack:** the surface starts as its background colour. Its enabled layers are blended in order, bottom first; when any enabled layer is solo, only solo layers draw. The result is multiplied by `master` and clipped to 0–255.
- **Blend modes,** on the 0–255 scale, per channel and only inside the layer's region:
  - `max`: the larger of the layer and what is below (today's behaviour)
  - `add`: their sum, at most 255
  - `multiply`: below × layer / 255
  - `alpha`: the layer over what is below, with its brightest channel / 255 as its opacity: layer + below × (1 − opacity). Black is transparent and a full colour opaque.

## Publishing

- The engine renders at the input rate: about 100 Hz for audio, the capture rate for screen.
- It publishes at the rate and resolution the controller requests per zone. Canvases default to 64×36 and are capped at 128×72 on the network; LZ4 compression is optional.
- **Rate:** a surface's newest frame is sent once its next send is due. Sends fall due every `1 / rate_hz` seconds, counted from the first, so a 60 Hz surface fed 100 frames a second still sends 60. After a pause longer than one period the count starts again. Frames aren't interpolated, and a surface rendering slower than its rate sends every frame.
- **Resolution:** a surface rendered at another resolution than requested is resampled by area: each requested cell is the average of the rendered cells under it, weighted by how much of each it covers, across the columns and then down the rows. At the same resolution the frame is sent as it is.
- **Quantisation** to the requested pixel format follows the [source link](../specs/source-link.md#surface-frames).
- The link and framing are in the [source link specification](../specs/source-link.md).

## Controller stages

1. **Intake:** the newest frame per source surface wins.
2. **Compositor:** per surface, a priority list of sources.
   - A source is active when it reports activity, for example audio above the silence threshold within the last 2 s.
   - The highest-priority active source wins, with a 300 ms crossfade on change.
   - Blending all active sources is a per-surface option.
3. **Reactive mode and ambient light** (per zone):
   - **On:** the sources' surfaces are shown.
   - **Off:** the ambient light is shown.
   - **Auto:** the sources' surfaces are shown while they are active; the ambient light is shown after `idle_after` seconds of silence (1–600 s, default 5 s), or after 0.3 s of silence when the source's player is paused and the zone enables "on pause".
   - **The ambient light** is a colour × level, solid or breathing (30–100% over 8 s). It fades in over 1.5 s (eased) and back out over 0.15 s, so the first beat after a pause still shows.
   - It replaces the earlier "idle scene or black" fallback, and it shows even when no source is connected.
4. **Projection:** gather plans per (surface, target) mapping ([spatial model](spatial-model.md)).
5. **Adapters:** [capabilities](capabilities.md).
6. **Providers:** one sender per provider instance at its own rate.
   - **Only changed frames** are sent, plus a keepalive inside the device's timeout (Hyperion 0.5 s; WLED under its 2.5 s realtime timeout; Hue at least every 5 s).
   - A safety timeout releases devices if FOLO stops.

## Parity mode

Configurations migrated from the current studio map to:
- one strip surface whose length equals the old LED count
- one line target on the Hyperion provider, with a manual 1:1 mapping
- an identity adapter chain

**What must match:**
- **Reactive output:** the bytes sent to Hyperion **must equal** the current engine's output for the same audio.
- **Ambient light:** frames during ambient-light fades and breathing, now computed by the controller, must match within ±1 per channel ([ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)).

Golden-frame tests enforce both ([testing policy](../process/testing-policy.md)).

## Performance budget

Measured with the current Python engine (after its efficiency work), one 10 ms audio packet per row. Synthetic music; beat tracker not running (it adds about 50–70 µs while it runs):

| Setup | Mean | p99 |
|---|---|---|
| 150 LEDs, 3 segments | 0.27 ms | 0.44 ms |
| 300 LEDs, 12 segments | 0.54 ms | 0.92 ms |
| 1000 LEDs, 30 segments | 1.24 ms | 1.91 ms |
| 2000 LEDs, 64 segments (schema maximum) | 2.42 ms | 7.80 ms (rare outliers well above 10 ms) |

**Controller:** compositing, projection, gamma and quantisation together take about 0.8 ms for 5000 elements × 9 samples from 8 sources (numpy reference; the Rust implementation must not be slower).

**Rules:**
- The effect engine must render at least 50 Hz in the worst case. When it can't keep up, it skips frames rather than queueing them.
- Profiling hot spots may move to Rust through PyO3 without changing the process model.
