# ADR-0023: Legacy code and licensing

Status: Accepted
Date: 2026-10-13

## Context

- The repository carries the upstream effect runner (`main.py`, `effects/`, `lib/`, …), which the studio doesn't use.
- The repository is MIT-licensed (upstream © Juha Rantanen).
- The six moving modes and 18 effects were ported with reference to LedFx's source, and LedFx is GPL-3.0. Distributing translated GPL code in MIT binaries would be a licence violation.

## Decision

- **D1.** Tag the state before the rebuild as `v0.9.0-legacy`, then **remove the legacy effect runner from `main`**. History and the tag keep it available.
- **D2.** **FOLO stays MIT.**
- **D3.** Before the first public binary release, the LedFx-inspired modes and effects are **rewritten clean-room**:
  - First, behaviour specifications are written from observation and documentation, without reading LedFx code.
  - The implementer works from those specifications only.
  - The content keeps "LedFx-inspired" attribution in descriptions.
- **D4.** Third-party code copied into FOLO is limited to compatible licences and recorded in `THIRD_PARTY_NOTICES`. Never copied: diyHue's `Lights/` (CC BY-NC) and Signify's documentation.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Relicense FOLO as GPL-3.0 | No rewrite | Changes the project's licence; limits reuse |
| Keep the ported code as is | No work | Licence risk in distributed binaries |
| Move legacy code to `legacy/` | Still visible | Dead code to lint and maintain |

## Consequences

A run book step covers the clean-room rewrite. The release checklist blocks the first public binary until it's done.
