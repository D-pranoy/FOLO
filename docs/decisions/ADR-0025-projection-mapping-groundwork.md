# ADR-0025: Projection-mapping groundwork

Status: Accepted
Date: 2026-10-13

## Context

Projection mapping is a long-term goal and out of scope for version 1. Adding it later should cause as little rework as possible.

## Decision

- **D1.** Version 1 includes the groundwork items G1–G8 in [projection-mapping groundwork](../architecture/projection-mapping-groundwork.md):
  - metric zone coordinates
  - a reserved `display` target kind with warp and mask
  - typed frames and pixel formats
  - an abstract projector
  - `RasterOutput`
  - end-to-end timestamps
  - extensible calibration
  - resolution-independent surfaces
- **D2.** Nothing in version 1 executes display targets or warps.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| No groundwork | Less work now | Schema and interface churn later |
| Implement projection mapping now | Complete | Large scope (GPU, warp editor, calibration) |

## Consequences

Schemas validate `display` targets. The projector is an interface from day one.
