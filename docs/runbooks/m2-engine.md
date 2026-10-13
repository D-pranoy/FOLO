# M2 Engine

Status: Accepted · Implementation: Not started

## RB-03: Effect engine restructure

**Goal:** the current Python engine becomes the `folo-source` package without changing behaviour.
**Depends on:** RB-01.

1. **Package:** create `source/` (Python package `folo_source`) and move code into it:
   - `inputs/audio/analyzer.py`: Analyzer, Spectrum
   - `inputs/audio/devices.py`: device lookup, COM initialisation
   - `effects/visualizers.py`: Renderer logic, SegmentState, modes
   - `runtime/`: process lifecycle

   Golden tests must still pass.
2. Keep `app/studio_config.py` as `folo_source/legacy/v1.py`, the reference for migration and the clamp corpus.
3. **Logging:** stdlib `logging` with JSON lines for the node to collect. Replace every `print`.
4. **Lifecycle:** clean shutdown on SIGINT, SIGTERM and Windows console control events; join threads with timeouts; seedable randomness.
5. **Settings:** received from the node (command line or loopback handshake); no settings file of its own.
6. **Server bugs:** fix the defects listed in [current system](../architecture/current-system.md#known-defects-to-fix-in-the-rebuild) that live in shared code (encoding, joining threads).
7. The legacy studio (`studio.py`, `web/`) keeps working, importing from the new package, until M3 and M4 replace it.

**Done when:** golden tests are unchanged and the legacy studio still runs.

## RB-04: Source pipeline and parity

**Goal:** inputs → effects → surfaces → publisher, with byte-exact parity.
**Depends on:** RB-02, RB-03.

1. **Core types** from the generated models: Frame, Signal, PixelFormat; a monotonic µs clock.
2. **Inputs:**
   - an interface
   - `AudioInput` (capture loop + Analyzer, exposing full and fast spectra and `band_levels`)
   - `GeneratorInput` (time, LFO, noise)
   - a `ScreenInput` placeholder (filled in RB-14)
   - input listing: every available audio device and screen, with enable flags
3. **Effect interface:** id, version, parameter schema, inputs, minimum capabilities; a registry. Port the per-segment logic as the `audio-visualizer` effect, with state per layer.
4. **Surfaces:** a renderer with layer stacks (max blend by default, more blend modes), master and background; strips and canvases; axis modes `linear_x`, `linear_y`, `radial` and `angular` for 1D effects on canvases.
5. **Post-processing layers:** brightness/saturation, mirror, blur, trail.
6. **Publisher:**
   - quantises surfaces to the requested pixel format with the same rounding as today's engine
   - resamples to the requested resolution and rate
   - optional LZ4
   - sends frames to the node over loopback using the [source link frame format](../specs/source-link.md#surface-frames), plus activity signals
7. **Executor:** dependency order, double-buffered surfaces, per-layer timing, a budget that skips frames below 50 Hz instead of queueing.
8. **Parity gate:** migrated configurations through the new pipeline (strip surface, 1:1 cells) must reproduce the RB-01 golden frames byte for byte.

**Done when:** the parity tests and pipeline unit tests are green.

## RB-05: v1 configuration migration

**Goal:** importing the current `studio.json` into the v2 documents.
**Depends on:** RB-02, RB-04.

1. Implement the mapping in [configuration model](../specs/config-model.md#migration-from-the-current-studiojson-version-1) as `folo-source migrate-v1 <studio.json> <out-dir>`. It writes the zones, show, providers and library documents and keeps a backup.
2. Running it more than once must give the same result. Built-in `default_preset` ids resolve through the alias table (filled in RB-09).
3. **Golden migration tests:** sample v1 files (default config, every built-in applied, saved presets, Hyperion disabled) → v2 documents that pass schema validation → rendered through RB-04 to equal the golden frames.
4. The node calls the tool on first run when a legacy `studio.json` is found (wired in RB-06).

**Done when:** migration goldens are green and the output validates against the schemas.
