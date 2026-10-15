# M2 Engine

Status: Accepted · Implementation: Partial (RB-03 done; RB-04 steps 1–3 done)

## RB-03: Effect engine restructure

**Goal:** the current Python engine becomes the `folo-source` package without changing behaviour.
**Depends on:** RB-01.

1. **Package:** create `source/` (Python package `folo_source`) and move code into it:
   - `inputs/audio/analyzer.py`: Analyzer, Spectrum, AutoLevel
   - `inputs/audio/windows/`: the WASAPI capture (`wasapi.py`, `audio_input.py`), including app capture and device following
   - `signals/beat.py`: BeatTracker
   - `inputs/media/windows.py`: now playing through `winrt`
   - `effects/visualizers.py`: Renderer logic, SegmentState, modes, beat targets
   - `runtime/`: process lifecycle, idle rendering

   The ambient light stays in the legacy path until RB-08 moves it to the controller. Golden tests must still pass.
2. Keep `app/studio_config.py` as `folo_source/legacy/v1.py`, the reference for migration and the clamp corpus.
3. **Logging:** stdlib `logging` with JSON lines for the node to collect. Replace every `print`.
4. **Lifecycle:** clean shutdown on SIGINT, SIGTERM and Windows console control events; join threads with timeouts; seedable randomness.
5. **Settings:** received from whoever runs the engine; no settings file of its own. Until the node starts it with its settings on the command line or over a loopback handshake ([RB-06](m3-controller-core.md#rb-06-node-core)), the legacy studio hands over its config store.
6. **Server bugs:** fix the defects listed in [current system](../architecture/current-system.md#known-defects-to-fix-in-the-rebuild) that live in shared code (encoding, joining threads).
7. The legacy studio (`studio.py`, `web/`) keeps working, importing from the new package, until M3 and M4 replace it.

**Done when:** golden tests are unchanged and the legacy studio still runs.

## RB-04: Source pipeline and parity

**Goal:** inputs → effects → surfaces → publisher, with byte-exact parity.
**Depends on:** RB-02, RB-03.

1. **Core types** from the generated models: Frame, Signal, PixelFormat; a monotonic µs clock.
2. **Inputs:**
   - an interface
   - **`AudioInput`:**
     - capture plus the Analyzer, exposing full and fast spectra and `band_levels`
     - Windows: output loopback, inputs and app capture (only / except)
     - Linux: PulseAudio/PipeWire monitors and inputs
     - gain and auto level; quiet-window skip; stats and meters
   - **`MediaInput`:** Windows media sessions through `winrt`; Linux MPRIS through `dbus-fast`. Album art, cover colours, play state, controls.
   - `GeneratorInput` (time, LFO, noise)
   - a `ScreenInput` placeholder (filled in RB-14)
   - input listing: every available output, input, app playing audio, screen and media session, with enable flags
3. **Beat signal:** the beat tracker runs on every analysed packet while needed, and is published as a signal. The `beat` trigger and `beats` are implemented in the audio-visualizer effect, with the zone's beat offset.
4. **Effect interface:** id, version, parameter schema, inputs, minimum capabilities; a registry. Port the per-segment logic as the `audio-visualizer` effect, with state per layer, the per-configuration preparation, and album-colour palette overrides.
5. **Surfaces:** a renderer with layer stacks (max blend by default, more blend modes), master and background; strips and canvases; axis modes `linear_x`, `linear_y`, `radial` and `angular` for 1D effects on canvases.
6. **Post-processing layers:** brightness/saturation, mirror, blur, trail.
7. **Publisher:**
   - quantises surfaces to the requested pixel format with the same rounding as today's engine
   - resamples to the requested resolution and rate
   - optional LZ4
   - sends frames to the node over [the local link](../specs/source-link.md#the-local-link) using the [source link frame format](../specs/source-link.md#surface-frames), through `websockets`, with `lz4` for compression ([ADR-0032](../decisions/ADR-0032-local-source-link.md))
   - also sends activity (with seconds of silence), beat signals, and media and input notifications
8. **Executor:**
   - dependency order, double-buffered surfaces, per-layer timing
   - a budget that skips frames below 50 Hz instead of queueing
   - idle rendering at 30 fps when no audio packets arrive
9. **Parity gates:**
   - Migrated configurations through the new pipeline (strip surface, 1:1 cells) reproduce the RB-01 golden frames byte for byte, including beat-triggered layers.
   - Beat-tracker goldens: synthetic songs, beats within ±70 ms, bar starts, no lock on beatless music.
   - Quiet-window outputs equal full-FFT outputs.

**Done when:** the parity tests, beat goldens and pipeline unit tests are green.

## RB-05: v1 configuration migration

**Goal:** importing the current `studio.json` into the v2 documents.
**Depends on:** RB-02, RB-04.

1. Implement the mapping in [configuration model](../specs/config-model.md#migration-from-the-current-studiojson-version-1) as `folo-source migrate-v1 <studio.json> <out-dir>`. It covers audio input, reactive mode, ambient light, beat offset and now-playing settings, writes the zones, show, sources, providers and library documents, and keeps a backup.
2. Running it more than once must give the same result. Built-in `default_preset` ids resolve through the alias table (filled in RB-09).
3. **Golden migration tests:** sample v1 files (default config, every built-in applied, saved presets, Hyperion disabled) → v2 documents that pass schema validation → rendered through RB-04 to equal the golden frames.
4. The node calls the tool on first run when a legacy `studio.json` is found (wired in RB-06).

**Done when:** migration goldens are green and the output validates against the schemas.
