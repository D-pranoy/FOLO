# ADR-0002: Product principles and scope

Status: Accepted
Date: 2026-10-13

## Context

FOLO started as an audio-reactive LED studio for Hyperion on one Windows PC. The goal is now a complete, vendor-neutral lighting-management system. That ambition needs explicit principles and a bounded first version.

## Decision

- **D1.** The product vision and principles in [vision.md](../vision.md) are adopted:
  - documentation first
  - setup-agnostic
  - simple by default with full granularity on demand
  - local-first and LAN-only
  - lighting data rather than media leaves a machine
  - safe by default
  - touch-first
  - contracts first
- **D2.** Version 1 is bounded by [scope.md](../scope.md). Work beyond it follows the [roadmap](../roadmap.md).
- **D3.** No telemetry, accounts or cloud services.
- **D4.** macOS is not supported. Windows and Linux are, in the tiers of [ADR-0018](ADR-0018-packaging-and-supported-platforms.md).
- **D5.** The roadmap gives order and dependencies only, never durations.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Everything at once | Complete earlier on paper | Unbounded risk; nothing releasable for a long time |
| Stay a single-machine studio | Small | Doesn't meet the goal |

## Consequences

Feature requests are checked against these principles and the scope. Out-of-scope items go to the roadmap, not into version 1.
