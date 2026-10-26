# ADR-0036: The controller's output: zones, projection and adapters

Status: Accepted
Date: 2026-10-26

## Context

RB-08 builds the controller half of the pipeline: surfaces are composed per zone, projected onto targets, adapted to each target's capabilities and handed to providers ([pipeline](../architecture/pipeline.md#controller-stages)). The decisions that shape it ([ADR-0005](ADR-0005-pipeline-and-data-model.md), [ADR-0006](ADR-0006-spatial-model-zones-and-projection.md), [ADR-0007](ADR-0007-capabilities-adapters-and-flash-limiter.md), [ADR-0033](ADR-0033-flash-limiter-off-by-default.md)) leave open what an implementation has to choose:

- **Stage order.** ADR-0007 D2 lists the adapter chain as resolution → colour model → calibration → rate → latency offset → flash limiter. Rate belongs to the sender: it drops unchanged frames, repeats frames for keepalive and paces state targets. A limiter that runs after rate would limit frames the device never gets, and a delay line after rate would delay the pacing too.
- **The ambient light and parity.** The current engine blends the ambient light **before** `master` ([current system](../architecture/current-system.md)), and the new engine's frames already carry `master`. The ambient golden frames have to match within ±1 per channel ([parity mode](../architecture/pipeline.md#parity-mode)), which fixes where the controller blends it.
- **What the specifications leave out:**
  - which of two zones that share a target runs when both are active at start, and what "shared" means
  - what silence, pause and "no active source" mean for a zone
  - where a zone's origin is, where line and grid elements sit, what the kernels compute, how several mappings combine, and what a new zone gets by default
  - what each adapter stage computes, and the exact rule of the flash limiter
  - what `target:<id>` frames carry, the test patterns, and the controller's clock
- **What the code has today.** The node answers `activate`, `deactivate`, `identify`, `test`, `discover` and `admin` with `501`, reports every zone inactive and every provider `off`, and the supervisor keeps the newest raw frame per surface without waking anyone when one arrives ([control plane](../architecture/control-plane.md#supervision)).

## Decision

This supersedes **ADR-0007 D2's order of stages** only. ADR-0007's other decisions stand, and ADR-0033 still governs the flash limiter's default and its warnings.

**Order and clock**

- **D1.** **Stage order.** For each target, in this order: **resolution → colour model → calibration → latency → flash limiter**. Then the provider's **sender** applies the rate: it sends only changed frames, repeats the last frame as a keepalive, and paces state targets with a token bucket ([providers](../architecture/providers.md#the-sender)). The stages are specified in [capabilities](../architecture/capabilities.md#adapter-chain-per-target-in-order).
- **D2.** **A steppable clock.** The controller's output core is a `step(now_us)` that takes the time as an argument. The node calls it when a frame arrives and at least every 1/60 s. Tests drive it with their own clock, so the compositor's crossfade, the ambient fades and the delay line are deterministic.

**Zones and activation**

- **D3.** **Activation is stored.** A zone has `active` (boolean, default true). When the node starts with two active zones that share a target, the one that comes first in `zones.json` keeps it; the later one reports `active: false` on the live channel, with a warning in the log. The file isn't rewritten.
- **D4.** **A shared target** is the same target id **or** an overlapping `(provider instance, address range)`: overlapping LED ranges, or at least one Hue channel in common.
- **D5.** **`take_over`.** `POST /zones/{id}/activate` with `take_over: true` deactivates the conflicting zones entirely and activates this one. Without it, a conflict is answered `200` with `{active: false, conflicts}` and nothing changes. The new state is written to `zones.json`.
- **D6.** **The engine renders only active zones.** `config.apply` assigns the surfaces of active zones and no others ([control plane](../architecture/control-plane.md#supervision)).

**Reactive output and the ambient light**

- **D7.** **Ambient before projection.** The controller blends the ambient light into the **composited surface**, before projection. The ambient colour is `colour × level × surface.master`, and the composite is mixed towards it by the fade amount. The composite already carries `master`, so the result equals the current engine's `mix(segments, ambient colour × level) × master`. A fade of 0 leaves the frame untouched, byte for byte. The result is truncated to `rgb8`. This is the only order that stays within ±1 per channel of the current engine's ambient frames.
- **D8.** **Silence and pause per zone.**
  - A zone is **quiet** while none of the sources feeding it reports `active`. The controller counts `quiet_s` on its own clock and doesn't use the sources' `quiet_s`.
  - It is **paused** when every linked source of the zone that has media reports `paused`.
  - A zone with no source counts as quiet from the moment it starts, so the ambient light shows ([pipeline](../architecture/pipeline.md#controller-stages)).
- **D9.** **No active source.** The zone shows the highest-priority source that sent a fresh frame (at most 500 ms old) even though it isn't active, and otherwise the surface's background × `master`. This keeps the idle glow of a segment (`floor`) visible in silence, as it is today.

**Projection**

- **D10.** **The zone's origin** is the centre of the floor: the box spans `x, y ∈ [−b/2, b/2]` and `z ∈ [0, bz]`, where `b` is the zone's bound on that axis. The reference object keeps its own position.
- **D11.** **Lines and grids** place their elements as the [spatial model](../architecture/spatial-model.md#element-positions) says: element `i` of a line of `N` at arc length `i / (N − 1)`, explicit `positions` winning when there are exactly `count`; a path with fewer than 2 points is unplaced; a grid's elements follow its wiring order.
- **D12.** **Kernels and mappings** are those of the [spatial model](../architecture/spatial-model.md#sampling-kernels):
  - **nearest:** the cell the sample falls in; **bilinear:** between cell centres
  - **area:** a box of half-size `radius` × the element's spacing, with exact overlap weights, at most 64 taps, and larger boxes sampled on an 8×8 sub-grid
  - **edge** `clamp` clamps a tap to the surface; `none` drops taps outside it and renormalises
  - **manual ranges** sample at `(i + 0.5) · cells / elements`, and **equal counts give one tap of weight 1**, which is what makes parity exact
  - several mappings onto one target blend in ascending priority, ties in document order, by max or alpha; elements no mapping covers are black; `display` targets are skipped
- **D13.** **Defaults** are applied only when a zone is created through the API or a target is added, never at load or migration:
  - surfaces `canvas.room` (64 × 36, on the floor plane, row 0 on the +y side), `canvas.screen` (64 × 36, on the TV's plane) for `screen` zones, and `strip.<target>` for each line target
  - one default mapping per new target
  - a new surface is fed by the source `local`
  - the reference object has a size, default 1.2 × 0.68 m

**Adapters**

- **D14.** **Two adapter fields** the descriptors lacked: `calibration.white` (`off`, `accurate` or `brighter`; default `accurate`) selects the white extraction for RGBW and RGB+CCT targets, and `calibration.onoff` (`threshold` 0.5, `hysteresis` 0.1, `min_interval_ms` 1000) sets the on/off targets' switching. Combining down to fewer elements stays a mean. On CCT targets a fixed rule maps saturated colours to the warm or the cool white.
- **D15.** **Calibration** is exactly this, per element and channel, on the 0–255 scale:
  - **gamma:** `255 · (v / 255)^γ`
  - **white balance:** each channel times its factor
  - **brightness range:** non-zero values map into `[min, max]` of the full scale; zero stays zero
  - **colour order:** a permutation of the channels
  - A target whose calibration holds only defaults skips the stage, which is how parity stays byte-exact.
- **D16.** **Resolution** fits the projected elements to the target's `address.count`: a mean when there are more elements than addresses, repetition when there are fewer.
- **D17.** **Latency.** The delay line holds each target's frames by `calibration.latency_ms` plus the difference between the largest `limits.latency_ms` among the zone's targets and the target's own, between 0 and 5 s. Faster outputs therefore wait for the slowest.
- **D18.** **The flash limiter's algorithm** (for zones that switched it on, [ADR-0033](ADR-0033-flash-limiter-off-by-default.md)):
  - It works per element, on relative luminance (Rec. 709 weights on linear light); for colour models without RGB, on the brightness component.
  - A **transition** is a change of at least 0.10 in luminance where the darker side is below 0.80. A **flash** is two opposing transitions.
  - Each element keeps a sliding window of 1 s. A transition that would make a fourth flash in the window is held: the element keeps its brightness until the window allows it.
  - On/off targets hold their state instead.
  - Test patterns go through it as any frame does.

  This is how WCAG 2.3.1's limit of three flashes a second is put into code. A limiter that fails to act while it is on counts as a security problem (ADR-0033 D6).

**Live data and test patterns**

- **D19.** **`target:<id>` frames** carry the projected RGB after the ambient light and before the adapters: `rgb8`, width = the element count, height 1.
- **D20.** **Test patterns** (`POST /zones/{id}/test`, and `identify`):
  - `identify`: white at 25%, pulsing at 1 Hz
  - `sweep`: one lit element moving along the elements' order
  - `position`: each element's x, y and z as R, G, B
  - The answer is `202`. A target another active zone drives gets `409 target_busy`.

**Tests and scope**

- **D21.** **Goldens.**
  - Projection images are PNGs in `tests/golden/projection/`, written only when `FOLO_BLESS=1` is set.
  - The ambient inputs come from a new part of `scripts/make_golden.py` (`--only ambient`); the expected output is the **existing** renderer PNGs, so no golden is regenerated.
  - The parity test uses the reactive `renderer/*.png` frames.
- **D22.** **LZ4 is deferred** to RB-16. The controller drops an LZ4-compressed frame from a source with a log line; loopback frames aren't compressed ([source link](../specs/source-link.md#surface-frames)).

The provider interface that these stages feed is in [providers](../architecture/providers.md).

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Keep ADR-0007's order, rate before latency and limiter | No supersession | The limiter would judge frames the sender may drop or repeat, and the delay line would delay pacing |
| Ambient light blended after projection | Simpler: the ambient light is per target | Misses the ambient goldens by more than ±1 per channel, because the current engine blends before `master` |
| Ambient light blended inside the effect engine | Same as today | The engine can't know the zone's reactive state across sources, and sources would each need the zone's settings |
| Activation held only in memory | No file field | Which zone drives a shared target after a restart would depend on request order |
| Reject a conflicting activation | Simplest | The UI needs the conflicting zones to ask the user before taking them over |
| Limiter on average luminance of a whole target | Cheap | A single bright element flashing alone would pass |
| A thread timer for the output core | Independent of frame arrival | Not deterministic in tests; Windows timers tick at about 15.6 ms |
| LZ4 in the controller now | Remote sources compress | No remote source exists before RB-16 |

## Consequences

- **Documents that change:**
  - [pipeline](../architecture/pipeline.md): the controller's stages, the ambient light, silence and pause, the idle fallback
  - [capabilities](../architecture/capabilities.md): the chain's order and what each stage computes, and the limiter's algorithm
  - [spatial model](../architecture/spatial-model.md): activation, the origin, element positions, kernels, mappings and defaults
  - [providers](../architecture/providers.md) (new), [control plane](../architecture/control-plane.md#supervision) and the [client API](../specs/client-api.md)
  - [M3](../runbooks/m3-controller-core.md) and [M5](../runbooks/m5-devices.md): RB-10, RB-12 and RB-13 depend on RB-08 step 6
  - [ADR-0007](ADR-0007-capabilities-adapters-and-flash-limiter.md): the status line
- **Contracts, in a later change** (additive; the configuration's `schema_version` stays 2, the client API's version goes up):
  - `Zone.active`, `Reference.size`, `calibration.white` and `calibration.onoff`
  - the error codes `target_busy` and `provider_error`, if the contract fixes them as an enumeration
  - the configuration model's tables, and its text for line paths and grids
  - the clamp corpus and the migration goldens follow, which regenerates existing golden files and needs a maintainer's approval
- **Safety:** the limiter's algorithm (D18) is a safety decision. Whether it works on real lights is checked on devices ([testing policy](../process/testing-policy.md)).
- **Rejecting D7** breaks the ambient golden tests: they would need a new tolerance, which parity doesn't allow.
- **Not covered by CI:** hardware parity (RB-10), the limiter on real lights, Windows timer granularity against a 40–60 Hz sender, and the 0.8 ms performance budget on real hardware.
