# ADR-0007: Capabilities, adapters and the flash limiter

Status: Accepted · D2's order of stages superseded by [ADR-0036](ADR-0036-controller-output.md), D4 superseded by [ADR-0033](ADR-0033-flash-limiter-off-by-default.md)
Date: 2026-10-13

## Context

Targets range from individually addressable RGB strips to single white-only lamps, relays, RGBW and CCT strips, and Hue lights with colour gamuts. Some are streamed at 50–60 Hz; others accept a few commands per second. Downloaded content and screen content can flash.

## Decision

- **D1.** Every target has a **capability descriptor**:
  - colour model: on/off, dimmer, CCT, RGB, RGBW, RGB+CCT, xy-gamut
  - addressing
  - delivery: streaming or state
  - limits

  Providers discover it where possible, and the user can override it.
- **D2.** A per-target **adapter chain** runs in this order: resolution → colour model → calibration → rate → latency offset → flash limiter ([capabilities](../architecture/capabilities.md)).
- **D3.** Effects declare minimum capabilities. On smaller targets, the engine renders at a virtual resolution and combines down.
- **D4.** **The flash limiter is on by default**, limiting large luminance transitions to at most 3 per second (WCAG 2.3.1). A zone may opt out, with a recorded photosensitivity warning.
- **D5.** Parity mode uses an identity adapter chain.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Treat everything as RGB | Simple | Wrong colours on white-only and RGBW lights; floods slow devices |
| No flash limiter | Simpler | Photosensitivity risk, especially with third-party content |

## Consequences

Providers must report capabilities. The UI shows capability badges in the zone setup.
