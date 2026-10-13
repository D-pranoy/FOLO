# Spatial model, zones and projection

Status: Accepted · Implementation: Not started

Decided in [ADR-0006](../decisions/ADR-0006-spatial-model-zones-and-projection.md).

## Zones

A **zone** is a spatial setup: a room, a TV corner, a desk. A controller has any number of zones. Each zone has:

- a **coordinate space**: right-handed, in **metres**
  - **x** points right, **y** points forward (towards the reference object), **z** points up
  - the origin is the zone's reference point
- **bounds**: the room dimensions, used to convert to and from normalised coordinates
- a **reference object**: TV, listening position, desk or none, with its position and orientation
- a **type**: Music, TV/Screen, Desk, Room or Custom. The type sets default surfaces and setup steps.
- its **targets**, **surfaces** and **mappings**
- an **active scene** and its **sources** (per surface, a priority list)

### Zone activation rules

- A target may be a member of several zones.
- **At any moment, at most one active zone drives a given target.** Activating a zone that shares targets with an active zone asks before taking them over.
- Zones that share no targets run at the same time.

## Targets

| Kind | Elements | Placement |
|---|---|---|
| `point` | 1 | A position |
| `line` | N | A polyline. Elements are spaced evenly by arc length, or given explicit positions (for example Hue gradient channels). |
| `grid` | W × H | A rectangle (origin, u and v axes, size) with a wiring order: row- or column-major, serpentine, origin corner |
| `display` | Resolution | Reserved for projection mapping: a rectangle plus `warp` (homography or mesh) and a mask. Validated but not executed in v1 ([groundwork](projection-mapping-groundwork.md)). |

Every target also has:
- a **provider binding**: which provider instance and device, and which address range (LED indices, Hue channel ids)
- a **capability descriptor** ([capabilities](capabilities.md))
- **calibration**: gamma, white balance, brightness limits, colour order and latency offset

Element positions come from manual placement in the zone editor, or are imported from provider data:
- Hue entertainment channel positions
- Hyperion LED rectangles around a screen
- WLED 2D matrix dimensions

## Surfaces

| Kind | Resolution | Placement |
|---|---|---|
| `strip` | N cells | A path (polyline) in the zone, or unplaced (manual mappings only) |
| `canvas` | W × H | A plane in the zone (origin, u and v vectors, size). The Screen surface sits on the TV's plane. |

Zone types create default surfaces with **roles**, which scenes bind to ([content](content.md)):
- every zone: a `canvas.room` floor-plan canvas covering the zone
- TV/Screen zones: also a `canvas.screen`
- each line target: a `strip.<target>`

## Mappings and projection

A **mapping** connects a surface to a target:

| Mapping | Sample coordinate per element |
|---|---|
| Plane (canvas) | Orthographic projection of the element position onto the plane, giving (u, v) |
| Path (strip) | The nearest point on the path, giving t |
| Manual | An explicit element range ↔ cell range, optionally reversed. Used for parity and unusual setups. |

**Sampling kernels:**
- nearest
- bilinear
- **area-average** with a radius: the default for canvases and screen content

Edge policy is clamp (ambilight-style; lights beside a screen sample its nearest edge) or none.

**Gather plans:**
- For each mapping, the controller precomputes index and weight arrays of shape `(elements, taps)` whenever the configuration revision changes.
- Each frame is then one vectorised gather and weighted sum.
- When several mappings feed one target, they blend in mapping priority order (max or alpha).

The projector is an interface (`Projector`) with a CPU gather implementation. A GPU implementation can be added for projection mapping without changing callers.

## Coordinate conversion with Hue

Hue entertainment positions are normalised to [-1, 1]: x right, y front, z up. FOLO converts using the zone bounds:

```
x_hue = 2 * (x - x_min) / (x_max - x_min) - 1   (likewise for y and z)
```

Positions are clamped to [-1, 1] on export. See [Hue](../integrations/hue.md#zone-sync).

## Zone setup experience

See [user interface](ui.md#zone-setup). Placement uses a top-down map with a height control. Lights can be dragged, strips drawn and grids placed. A live position test pattern runs on the real lights.
