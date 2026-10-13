# ADR-0021: Content packs

Status: Accepted
Date: 2026-10-13

## Context

People should be able to download and install effects and presets for many setups, at every level of detail. Today's built-ins are JavaScript code tied to one LED strip.

## Decision

- **D1.** Content levels: palette, reaction, effect, look, scene, zone template and pack ([content](../architecture/content.md)).
- **D2.** Scenes target **roles** with relative positions and are matched to zone surfaces automatically, with manual override and fallbacks.
- **D3.** v1 packs are **data only**:
  - validated and clamped
  - size-limited
  - imported and exported as `.folopack` ([pack format](../specs/pack-format.md))
  - distributed from the controller to sources by hash
- **D4.** The built-in library moves into packs shipped with the node. Reset all moves to the controller.
- **D5.** Applying copies the content and records its provenance, with "re-apply, keeping my changes" when a pack updates.
- **D6.** Recipe and expression effects, a signed community index and sandboxed WASM plugins come later.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Code plugins in v1 | Unlimited power | Needs a sandbox and a security review first |
| Live references with override layers | Updates flow automatically | Much more complex |
| A hosted marketplace | Best discovery | Servers, moderation, cost |

## Consequences

The UI's inspector is schema-driven. Pack installs need the `admin` scope.
