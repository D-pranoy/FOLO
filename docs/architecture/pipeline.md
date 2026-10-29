# Pipeline

Status: Accepted · Implementation: Partial (the effect engine's core types, inputs on Windows and Linux, the beat signal, the audio-visualizer and post-processing effects, surfaces, the publisher with the spectrum signal and layer levels, the local link and the executor; the controller's stages: intake, the compositor, the ambient light, projection, the adapters and the provider senders, stepped by the node's output task)

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
| Media session | Now playing: title, artist, album art, cover colours, play state; player controls. Windows media sessions or Linux MPRIS ([ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)). On Linux, art is read when the player names a local file (`file://`); art a player only has on the web isn't fetched. |
| Screen | One selected screen per source in v1, captured on demand by the capture agent ([display capture](../integrations/display-capture.md)). Its small frames reach the engine through the node, in their own message type ([source link](../specs/source-link.md#screen-frames)); they are never published. |
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
  - **Screen:** a canvas effect. It turns a captured frame into a picture for its layer's rectangle of a canvas: the black bars and margins cut away, sampled by area or from the picture's edges, with colour, saturation and smoothing ([display capture](../integrations/display-capture.md#the-screen-effect)). The video/game profile sets how the capture agent finds black bars.
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
- **Canvas effects** (the Screen effect) are a third kind, beside effects that draw a line and post-processing effects. They draw a 2D picture of rows × columns for their layer's rectangle of a canvas, and on a strip surface they draw nothing; projection maps canvases onto strips. Effect factories get a handle to the engine's inputs, which the Screen effect uses to read the newest screen frame ([ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D10).
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
- **Signals** the engine publishes when `config.apply` asks for them by name:
  - `beat`: `[bpm, confidence, locked, phase, bar_beat]` of the first surface's zone, while the beat tracker runs
  - `activity`: `[active (0 or 1), level_db, quiet_s]`, as the activity notification below
  - `spectrum`: 192 band levels for clients' analyzers ([source link](../specs/source-link.md#signal-frames))
- **Layer levels:** for each surface `config.apply` lists in `publish.levels`, the engine sends its layers' levels as the `levels` notification ([source link](../specs/source-link.md#source--controller-notifications)).
- The link and framing are in the [source link specification](../specs/source-link.md).

## The executor

The effect engine's executor renders the surfaces `config.apply` assigns and publishes them. Nothing is captured or rendered before the first `config.apply`; it starts the inputs.
- **Steps, in dependency order:** every analysed packet goes to the beat tracker and the activity; then each surface's layer stack is rendered, bottom first; then its frames and the signals are published.
- **Double-buffered surfaces:** a step renders into new frames and swaps them in as a whole, so the publisher never reads a half-rendered surface.
- **Timing:** each step times every surface and every effect in it (an effect draws all its layers of a surface in one go). The metrics notification carries the mean step time, and a step over the budget logs its slowest effect.
- **Budget:** rendering runs on a thread of its own. Packets that arrive while a frame renders become one step: each still reaches the beat tracker and the activity, and only the newest is rendered, with their time added up (at most 0.1 s). A slow frame therefore skips frames instead of queueing them.
- **Idle rendering:** after 0.1 s without an audio packet the executor renders silence at 30 fps, so releases, moving modes and the controller's ambient light keep time. While audio is idle, the arrival of a screen frame also starts a step, so Screen layers render at the capture rate; while audio plays, they render with the audio steps, with the newest frame ([ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D11).
- **Beat tracker:** it runs while an enabled layer uses the `beat` trigger or the `beat` signal is published; a new song from the media input makes it find the tempo afresh. Each beat offset among the assigned surfaces has its tracker.
- **Activity:** the audio is active while its loudest bin, without the input gain, reaches −70 dB. `quiet_s` adds up the seconds since it last was, as the ambient light counts them. It describes the whole source, so the notification leaves out `surface`.
  - **Screen activity:** each surface with an enabled Screen layer gets an `activity` notification of its own, with its `surface` and without `level_db`. It is active while the newest screen frame that is neither `held` nor `protected` is less than 0.5 s old, and `quiet_s` counts the seconds since it last was. A surface that also has enabled audio layers is active when either input is ([ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D12). On the secure desktop or for protected content the screen goes inactive, and the controller's ambient light crossfades in.
- **Layer state** (smoothing, moving modes, trails) is kept per surface index across `config.apply` calls.

## Controller stages

Decided in [ADR-0005](../decisions/ADR-0005-pipeline-and-data-model.md) and [ADR-0036](../decisions/ADR-0036-controller-output.md). The controller's output core is one `step(now_us)` that takes the time as an argument. The node runs it when a frame arrives and at least every 1/60 s, so the crossfade, the ambient fades and the delay line keep time without frames, and tests drive it with a clock of their own.

1. **Intake:** the newest frame per source surface wins; a late or duplicate sequence number is dropped. A compressed (LZ4) frame is dropped with a log line until RB-16 brings remote sources ([source link](../specs/source-link.md#surface-frames)). At each `config.apply` the controller drops every frame it holds by index, because the surfaces are numbered afresh, and takes frames again from the engine's answer on. Frames the compositor already took are held by surface id, which the renumbering doesn't change, so they stay until newer ones replace them or they go stale. When a source's link is made or ends, the compositor forgets everything that source sent.
2. **Compositor:** per surface, a priority list of sources.
   - For switching, a source is active when it reports activity, for example audio above the silence threshold within the last 2 s. A source that sends no frame for 500 ms counts as inactive. (The zone's silence below uses the activity notification as it is, without this hold.)
   - The highest-priority active source wins, with a 300 ms crossfade on change.
   - **With no active source,** the surface shows the highest-priority source whose newest frame is at most 500 ms old, active or not, so a layer's idle glow shows in silence. With no fresh frame at all, it shows its background × `master`.
   - Blending all active sources is a per-surface option.
3. **Reactive mode and ambient light** (per zone):
   - **On:** the sources' surfaces are shown.
   - **Off:** the ambient light is shown.
   - **Auto:** the sources' surfaces are shown while they are active; the ambient light is shown after `idle_after` seconds of silence (1–600 s, default 5 s), or after 0.3 s of silence when the zone's players are paused and the zone enables "on pause".
   - **Silence and pause:**
     - A zone is **quiet** while none of the sources feeding it reports `active` in its latest `activity` notification, which is instantaneous: the loudest bin, without gain, at or above −70 dB now. The controller counts `quiet_s` on its own clock and ignores the sources' own `quiet_s`.
     - It is **paused** when at least one linked source of the zone has a media session (state `playing`, `paused` or `stopped`) and all of those report `paused`. A zone fed only by sources without a session, such as a game or a line-in, is never paused.
     - A zone with no source at all is quiet from the moment it starts, so its ambient light shows.
     - A zone that becomes active starts reactive, with its silence counted from then on, and its targets' delay lines start empty, so frames from before it went off don't come out.
   - **The ambient light** is a colour × level, solid or breathing (30–100% over 8 s). It fades in over 1.5 s (eased) and back out over 0.15 s, so the first beat after a pause still shows.
   - **Where it is blended:** into each of the zone's composited surfaces, before projection. The ambient colour is `colour × level × master` of that surface (times the breathing factor, 0.3 to 1, for `breathe`), and the composite, which already carries `master`, is mixed towards it by the fade. That equals the current engine's `mix(segments, ambient) × master`, whose frames the golden tests compare within ±1. The result is truncated to `rgb8`. A fade of 0 leaves the frame untouched, byte for byte.
   - It replaces the earlier "idle scene or black" fallback, and it shows even when no source is connected.
4. **Projection:** gather plans per (surface, target) mapping ([spatial model](spatial-model.md)). The projected elements of a target, after the ambient light and before the adapters, are what `target:<id>` live frames carry: `rgb8`, as wide as the target has elements and one row high.
5. **Adapters:** per target, in the order resolution → colour model → calibration → latency → flash limiter ([capabilities](capabilities.md)).
6. **Providers:** one sender per provider instance at its own rate ([providers](providers.md)).
   - **Only changed frames** are sent, plus a keepalive inside the device's timeout (Hyperion 0.5 s; WLED under its 2.5 s realtime timeout; Hue at least every 5 s).
   - A safety timeout releases devices if FOLO stops.

## Parity mode

Configurations migrated from the current studio map to:
- one strip surface whose length equals the old LED count
- one line target on the Hyperion provider, with a manual 1:1 mapping (one tap of weight 1 per element, [spatial model](spatial-model.md#sampling-kernels))
- an identity adapter chain: a target whose calibration holds only defaults skips that stage ([capabilities](capabilities.md#adapter-chain-per-target-in-order))

**What must match:**
- **Reactive output:** the bytes sent to Hyperion **must equal** the current engine's output for the same audio.
- **Ambient light:** frames during ambient-light fades and breathing, now computed by the controller, must match within ±1 per channel ([ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)). Every frame is compared at ±1 on the controller's microsecond clock. A frame where the current engine's float sum of silence is within 1 µs of a switch threshold (the 0.3 s pause, `idle_after`), and the fade that follows it up to the frame after that fade settles, is compared one frame shifted: within ±1 of its input blended with the current engine's light of one step earlier, the same step or one step later. On such a tie the current engine switches or not on float rounding (it reads 0.3 at one and 0.9999999999999999 at another, both exact in microseconds), so a clock can land a step off it. The tie frames are worked out from the golden inputs' own float sums, not listed by hand, and the light's maths is held to ±1 on every frame when stepped in the current engine's seconds. (Decision to confirm.)

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
