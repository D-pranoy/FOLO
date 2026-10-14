# ADR-0024: Testing strategy and live-device safety

Status: Accepted
Date: 2026-10-13

## Context

The rebuild replaces almost every component, while existing behaviour must be kept. FOLO drives real lights in people's homes. Hosted CI runners have no lights, audio devices or displays.

## Decision

- **D1.** **Golden tests** pin behaviour:
  - renderer frames from deterministic synthetic audio (seeded randomness)
  - migration outputs
  - projection images
  - packet encoders
  - black-bar detection
- **D2.** **Fakes** stand in for every device in CI: a Hyperion JSON server, a WLED UDP receiver, a Hue bridge double (REST plus OpenSSL DTLS-PSK), synthetic and replayed audio and screen inputs, and a fake PipeWire node.
- **D3.** **Contract conformance tests** run against both the Rust and Python implementations.
- **D4.** **End-to-end UI tests** (Playwright) run against a node in demo mode, at phone, tablet and desktop sizes.
- **D5.** **Live-device verification** follows the [testing policy](../process/testing-policy.md):
  - never against someone's live configuration without their consent
  - announce changes before touching a device
  - low brightness first, one target at a time
  - release devices afterwards
- **D6.** CI is required to merge. Coverage is reported; new modules target at least 90% line coverage, excluding platform glue.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Manual testing only | Little setup | Regressions; unsafe on real devices |
| Hardware-in-the-loop CI | Real signal | Expensive, fragile; a later option |

## Consequences

Every device-facing run book builds its fake before its implementation.
