# Spatial model, zones and projection

Status: Accepted · Implementation: Not started

Decided in [ADR-0006](../decisions/ADR-0006-spatial-model-zones-and-projection.md) and, for activation, defaults and the details of projection, [ADR-0036](../decisions/ADR-0036-controller-output.md).

## Zones

A **zone** is a spatial setup: a room, a TV corner, a desk. A controller has any number of zones. Each zone has:

- a **coordinate space**: right-handed, in **metres**
  - **x** points right, **y** points forward (towards the reference object), **z** points up
  - **the origin is the centre of the floor**: the room's box spans `x, y ∈ [−b/2, b/2]` (`b` being the bound on that axis) and `z ∈ [0, bz]`
- **bounds**: the room dimensions, used to convert to and from normalised coordinates
- a **reference object**: TV, listening position, desk or none, with its position, orientation and size (a TV's width and height, 1.2 × 0.68 m by default). The reference object keeps its own position; it doesn't move the origin.
- a **type**: Music, TV/Screen, Desk, Room or Custom. The type sets default surfaces and setup steps.
- its **targets**, **surfaces** and **mappings**
- an **active scene** and its **sources** (per surface, a priority list)

### Zone activation rules

- A target may be a member of several zones.
- **At any moment, at most one active zone drives a given target.** Activating a zone that shares targets with an active zone asks before taking them over.
- Zones that share no targets run at the same time.
- **Shared targets:** two targets are shared when they have the same id, **or** drive overlapping `(provider instance, address range)` within one provider instance: overlapping LED ranges, or at least one Hue channel in common. Two instances that happen to reach the same physical device aren't detected.
- **Taking over:** activating a zone whose targets are shared with active zones answers with those zones as `conflicts` and changes nothing. With `take_over`, the conflicting zones are **deactivated entirely**, not only their shared targets, and the zone is activated ([client API](../specs/client-api.md#zone-activation)).
- **Activation is stored, the effective state is derived.** The zone's `active` flag (default true) is stored and survives restarts. The **effective** state is the stored flag minus the losers of this check: when active zones share a target, the one that comes first in `zones.json` keeps it, and each later one is effectively inactive, with a warning in the log. The check runs on every load and every write of a zone, so:
  - of the zones that share a target, the one first in `zones.json` keeps it, whether the write added a zone or changed one; a new zone goes last, so a new zone that shares a target with an active one comes up effectively inactive, and nothing is refused
  - a zone the check suppressed becomes effective as soon as the zone that held its target is deactivated or removed, as it would after a restart
  - the file's flag isn't rewritten by the check; `GET /zones` gives the stored one, and `ZoneActivation.active` and the live channel give the effective one
- **Details of the check:**
  - it goes through the zones in file order and a zone that lost holds nothing for the zones after it, so a chain of overlaps silences only the zones that must be
  - a target with no provider instance yet shares a target only by id; one bound to an instance without an address has the default address (LED 0, one LED)
  - Hue channels are compared with channels only, and LED ranges with LED ranges, so a channel target and a range target on one instance never share
  - `take_over` also deactivates the other zones that are active in the file and share a target, the suppressed ones included, so the zone can't lose a target to a zone that comes before it in the file; its answer has no `conflicts`, since none is left
- Only effectively active zones are rendered by the effect engine and driven by the controller ([control plane](control-plane.md#supervision)).

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

### Element positions

Where each element of a target sits, in the zone's coordinates:
- **point:** its position.
- **line** of `N` elements: element `i` is at arc length `i / (N − 1)` of the path (a line of one element is at the path's start). Explicit `positions` win when they have exactly `count` entries. A path of fewer than 2 points is **unplaced**: its elements have no position, and only manual mappings can reach them.
- **grid** of `W × H`: the rectangle is spanned from its `origin` corner by the edge vectors `u` (across the columns) and `v` (down the rows). Column `c` and row `r` are centred at `origin + u · (c + ½) / W + v · (r + ½) / H`. Element indices follow the **wiring order**: they start at the `start` corner, run along rows (or columns, with `order: column`), and each following row (column) runs the other way when `serpentine` is on.
- **display:** no positions; validated only.

## Surfaces

| Kind | Resolution | Placement |
|---|---|---|
| `strip` | N cells | A path (polyline) in the zone, or unplaced (manual mappings only) |
| `canvas` | W × H | A plane in the zone (origin, u and v vectors, size). The Screen surface sits on the TV's plane. |

Zone types create default surfaces with **roles**, which scenes bind to ([content](content.md)):
- every zone: a `canvas.room` floor-plan canvas covering the zone
- TV/Screen zones: also a `canvas.screen`
- each line target: a `strip.<target>`

### Defaults

Defaults are applied only when a zone is created through the API or a target is added to one, **never when a document is loaded or migrated**, so a migrated setup stays as the migration made it ([configuration model](../specs/config-model.md#migration-from-the-current-studiojson-version-1)):
- `canvas.room`: 64 × 36 cells on the floor plane, covering the room, with **row 0 on the +y side**: the plane's origin is the corner at `(−x/2, +y/2, 0)`, with `u` along +x and `v` along −y.
- `canvas.screen` (zones of type `screen` whose reference is a TV): 64 × 36 cells on the TV's plane, which comes from the reference object. At orientation 0 the TV faces the room: its centre is `reference.position`, its screen's normal points towards −y, and a viewer looking along +y sees +x to the right. Its rectangle has the origin corner at the top left as the viewer sees it, `(−w/2, 0, +h/2)` from the centre, with `u = (w, 0, 0)` and `v = (0, 0, −h)`, `w` and `h` being the reference's `size`. `orientation` then turns the rectangle about its centre: `yaw` about +z (counter-clockwise seen from above), then `pitch` about the TV's own x axis, then `roll` about its own y axis, in degrees. A zone whose reference isn't a TV has no `canvas.screen`. The default `reference.position` is `(0, 0, 0)`, the centre of the floor.
- `strip.<target>`: one strip surface per line target, with the target's element count as its resolution.
- One default mapping for each new target: a manual 1:1 mapping from its `strip.<target>` surface for a line target. For any other target, a plane mapping from `canvas.screen` in a `screen` zone with a TV (area kernel, edge `clamp`, so lights beside the screen sample its nearest edge), and from `canvas.room` otherwise.
- A new surface lists the source `local`.

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

### Sampling kernels

Each element has a sample position on its surface, in cell units. The kernels turn it into taps (a cell and a weight):
- **nearest:** the cell the sample falls in, with weight 1.
- **bilinear:** the four cells around the sample, weighted by their distance, measured between cell centres.
- **area:** a box centred on the sample, with half-size `radius` × the element's **spacing** along each axis of the surface. Each cell the box overlaps is a tap, weighted by its exact overlap, the weights normalised to add up to 1. At most **64 taps**: a larger box is sampled on an 8 × 8 sub-grid of the box, each sub-cell counting for the cell it falls in. `radius` is a fraction of the spacing, 0 to 1 (half a spacing by default).
  - **Spacing,** in cells of the surface: for a line, the distance between neighbouring elements (at the ends, to their one neighbour), converted to the surface's cells along the sample axis; for a grid, separately along `u` (the plane's width over `W`) and along `v` (its height over `H`), so the box can be a rectangle; for a point and for a line of one element, one cell; for a manual range, `cells / elements`.
  - **Box shape:** a strip surface has one row, so the box is that row's one cell tall and has its half-size only along the strip.
- **Edges:** with `clamp`, a tap outside the surface moves to the nearest cell. With `none`, taps outside are dropped and the remaining weights are renormalised to add up to 1.
- **Manual ranges:** element `i` of a range samples its cell range at `(i + ½) · cells / elements`. **When the two counts are equal the kernel is ignored:** every element has one tap of weight 1, which makes the mapping exact, whatever the kernel's type and radius. It is the parity mapping.

### Combining mappings

When several mappings feed one target:
- They blend in ascending `priority`, ties in the order of the zone's mapping list, by their `blend` (max or alpha).
- An element that no mapping covers is black.
- A mapping onto a `display` target is skipped.

**Gather plans:**
- For each mapping, the controller precomputes index and weight arrays of shape `(elements, taps)` whenever the configuration revision changes.
- Each frame is then one vectorised gather and weighted sum.
- When several mappings feed one target, they blend in mapping priority order (max or alpha).

The projector is an interface (`Projector`) with a CPU gather implementation. A GPU implementation can be added for projection mapping without changing callers.

## Coordinate conversion with Hue

Hue entertainment positions are normalised to [-1, 1]: x right, y front, z up. FOLO converts using the zone's box ([the origin](#zones) is the floor's centre, so `x_min` is `−bx/2`, `x_max` is `bx/2`, and `z_min` is 0):

```
x_hue = 2 * (x - x_min) / (x_max - x_min) - 1   (likewise for y and z)
```

Positions are clamped to [-1, 1] on export. See [Hue](../integrations/hue.md#zone-sync).

## Zone setup experience

See [user interface](ui.md#zone-setup). Placement uses a top-down map with a height control. Lights can be dragged, strips drawn and grids placed. A live position test pattern runs on the real lights.
