# Pipeline

Status: Accepted · Implementation: Not started

`inputs → effects → surfaces ⟶ compositor → projection → adapters → providers`

The first half runs in a **source** (the effect engine and capture agent). The second half runs in the **controller**. Decided in [ADR-0005](../decisions/ADR-0005-pipeline-and-data-model.md).

## Frames and signals

- **Frame:** an array of linear RGB values, shape `(N, 3)` for a strip or `(H, W, 3)` for a canvas, with metadata:
  - pixel format: `rgbf32` inside a process; `rgb8` or `rgb16` on the wire; a video format reserved
  - timestamp in monotonic microseconds
  - sequence number
- **Signal:** a named scalar or vector at a rate. Examples: band levels, an activity flag, external data.
- **Clock:** monotonic microseconds everywhere. Sources stamp frames, and the controller measures latency per source and per output.

## Inputs (in a source)

| Input | Description |
|---|---|
| Audio | WASAPI loopback (Windows) or a PulseAudio/PipeWire monitor (Linux), captured in about 10 ms packets. Analysed with 4096- and 2048-point Hann-windowed FFTs, as today (see [current system](current-system.md)). |
| Screen | One selected screen per source in v1, captured on demand by the capture agent ([display capture](../integrations/display-capture.md)) |
| Generator | Time, LFO, noise; for idle and ambient effects |
| Local ingest | Signals or frames pushed by local programs over loopback ([source link](../specs/source-link.md#local-ingest)) |

A source **lists every input available to it**. Each input is configured and enabled on its own. Inputs that are enabled but unused stay idle; display capture starts only when an active layer needs it.

## Effects

- An effect declares an id, a version, its inputs, a JSON Schema for its parameters, and minimum capabilities (for example, a minimum element count).
- **Built-in effects:**
  - **Audio visualizers:** the existing modes fill, pulse, spectrum, shift, scroll, drops, scan, wipe, flow and fire. Each layer has the existing segment semantics: band, trigger (level or hits), threshold, ceiling, gain, curve, attack, release, idle glow, direction, speed, size, peak hold and palette.
  - **Screen:** turns a captured frame into a canvas, with colour, saturation, smoothing and video/game profiles.
  - **Post-processing:** brightness and saturation, mirror, blur, trail.
- **1D effects on canvases** use axis modes: `linear_x`, `linear_y`, `radial` and `angular`. Every visualizer therefore works on 2D surfaces and in any zone.
- Effects run in-process in the effect engine. An out-of-process host is reserved for future plugins.

## Surfaces and layer stacks

- A surface has a kind (strip or canvas), a resolution, a background and a master level.
- Its layers form an ordered stack. The default blend is **max**, which reproduces today's behaviour; other blend modes are available per layer.
- A layer covers a **region** of the surface: a cell range on a strip, a rectangle on a canvas.

## Publishing

- The engine renders at the input rate: about 100 Hz for audio, the capture rate for screen.
- It publishes at the rate and resolution the controller requests per zone. Canvases default to 64×36 and are capped at 128×72 on the network; LZ4 compression is optional.
- The link and framing are in the [source link specification](../specs/source-link.md).

## Controller stages

1. **Intake:** the newest frame per source surface wins.
2. **Compositor:** per surface, a priority list of sources.
   - A source is active when it reports activity, for example audio above the silence threshold within the last 2 s.
   - The highest-priority active source wins, with a 300 ms crossfade on change.
   - Fallback is an idle scene or black. Blending all active sources is a per-surface option.
3. **Projection:** gather plans per (surface, target) mapping ([spatial model](spatial-model.md)).
4. **Adapters:** [capabilities](capabilities.md).
5. **Providers:** one sender per provider instance at its own rate, with a safety timeout so devices are released if FOLO stops.

## Parity mode

Configurations migrated from the current studio map to:
- one strip surface whose length equals the old LED count
- one line target on the Hyperion provider, with a manual 1:1 mapping
- an identity adapter chain

Through this path the bytes sent to Hyperion **must equal** the current engine's output for the same audio. Golden-frame tests enforce this ([testing policy](../process/testing-policy.md)).

## Performance budget

Measured with the current Python engine, one 10 ms audio packet per row:

| Setup | Mean | p99 |
|---|---|---|
| 150 LEDs, 3 segments | 0.40 ms | 0.64 ms |
| 300 LEDs, 12 segments | 1.13 ms | 1.97 ms |
| 1000 LEDs, 30 segments | 2.21 ms | 3.77 ms |
| 2000 LEDs, 64 segments (schema maximum) | 4.82 ms | 8.58 ms (max 13 ms) |

**Controller:** compositing, projection, gamma and quantisation together take about 0.8 ms for 5000 elements × 9 samples from 8 sources (numpy reference).

**Rules:**
- The effect engine must render at least 50 Hz in the worst case. When it can't keep up, it skips frames rather than queueing them.
- Profiling hot spots may move to Rust through PyO3 without changing the process model.
