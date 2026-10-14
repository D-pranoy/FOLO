# ADR-0005: Pipeline and data model

Status: Accepted
Date: 2026-10-13

## Context

Today's studio maps frequency-range "segments" onto one LED strip. The new system needs many inputs, composable effects, 2D content (screens) and several output devices. Existing behaviour must be kept exactly.

## Decision

- **D1.** The pipeline is `inputs → effects → surfaces → compositor → projection → adapters → providers` ([pipeline](../architecture/pipeline.md)).
- **D2.** Frames are typed: shape, pixel format, monotonic µs timestamp, sequence. `rgbf32` is used inside processes; `rgb8` or `rgb16` on the wire.
- **D3.** Effects declare a JSON Schema for their parameters, their inputs and minimum capabilities. The UI's inspector is generated from the schema.
- **D4.** Surfaces hold **layer stacks**. The default blend is max (today's behaviour). Today's segment becomes an audio-visualizer layer with an unchanged parameter set.
- **D5.** 1D effects run on canvases through axis modes (`linear_x`, `linear_y`, `radial`, `angular`).
- **D6.** Sources publish at the resolution and rate the controller requests per zone. Canvases default to 64×36 and are capped at 128×72 on the network; LZ4 is optional.
- **D7.** The compositor picks the highest-priority active source per surface, with a 300 ms crossfade and an idle fallback. Blending all active sources is a per-surface option.
- **D8.** **Parity mode:** migrated configurations must produce output byte-identical to the current engine through the new pipeline (strip surface, manual 1:1 mapping, identity adapters).
- **D9.** The effect engine must keep at least 50 Hz in the worst case, dropping frames rather than queueing them.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| A node-graph effect editor | Most general | Complex UI, against the simplicity principle; the internal model can still be a graph later |
| Keep fixed segments only | Simple | No 2D, no composition |
| Sources send analysis features; the controller renders | Rendering in one place | Contradicts the decision that sources run effects |

## Consequences

Golden-frame tests gate every change to the engine and the controller path ([ADR-0024](ADR-0024-testing-strategy-and-device-safety.md)).
