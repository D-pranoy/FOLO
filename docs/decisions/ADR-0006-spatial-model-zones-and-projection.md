# ADR-0006: Spatial model, zones and projection

Status: Accepted
Date: 2026-10-13

## Context

Lights differ in shape (lamps, strips, matrices) and sit in different rooms. Hue organises entertainment by zone (entertainment areas) with 3D positions. Screen sync needs 2D content mapped to lights around a screen. Projection mapping is a long-term goal.

## Decision

- **D1.** A controller has several **zones** (rooms, TV corners, desks). Each zone has a metric, right-handed coordinate space (x right, y forward, z up), bounds, a reference object, a type, targets, surfaces, mappings and an active scene.
- **D2.** **Targets** are `point`, `line` or `grid`. `display` is reserved for projection mapping.
- **D3.** **Surfaces** are `strip` (placed on a path) or `canvas` (placed on a plane). A manual mapping supports parity and unusual setups.
- **D4.** Projection uses **precomputed gather plans** (indices and weights per element) with nearest, bilinear or area-average sampling, behind a `Projector` interface. A CPU implementation comes now; GPU later.
- **D5.** A target may belong to several zones, but **one active zone drives it at a time**. Zones without shared targets run at the same time.
- **D6.** Zone types create default surfaces with roles (`canvas.room`, `canvas.screen`, `strip.<target>`) that scenes bind to.
- **D7.** Hue positions convert between metres and Hue's [-1, 1] using the zone bounds.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| One virtual strip with outputs mapped to slices | Smallest change | Wrong for lamps, white-only lights and screens |
| Surfaces concatenated per device, no space | Simple | No spatial effects; no Hue zone sync; no screen mapping |
| Full 3D volumes now | Most expressive | Not needed for v1; positions plus canvases cover it |

## Consequences

The zone editor is central to the UI ([ui](../architecture/ui.md#zone-setup)). Details: [spatial model](../architecture/spatial-model.md).
