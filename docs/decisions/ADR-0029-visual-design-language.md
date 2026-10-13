# ADR-0029: Visual design language

Status: Accepted
Date: 2026-10-13

## Context

The current studio adopted a glassmorphism look: translucent, blurred panels over fixed Deep Night glows, with controls sunk into the glass. It was built alongside measured performance rules that cut the page's CPU use to about a quarter. [ADR-0016](ADR-0016-user-interface.md) D8 keeps the current design as the base for the new UI. The design needs a precise, testable definition.

## Decision

- **D1.** The visual language of every FOLO screen is defined by the [visual design specification](../specs/visual-design.md):
  - glass panels over a fixed glow background
  - a surface hierarchy (panel, row, well, raised)
  - design tokens
  - colour semantics
  - typography
  - motion
  - focus and contrast
- **D2.** Tokens are defined once and exported as CSS variables for the UI and JSON for the desktop app.
- **D3.** The **performance rules** in the specification are normative:
  - drawing only on invalidation
  - no CSS animation at rest
  - offscreen caches for static layers
  - transform-only live bars
  - hidden pages closing the live stream
  - no nested backdrop filters
- **D4.** A **Reduce transparency** setting, and an automatic fallback where `backdrop-filter` isn't supported, replace glass with opaque surfaces.
- **D5.** Version 1 is **dark only** (`color-scheme: dark`).
- **D6.** Touch adaptations:
  - pressed states mirror hover
  - double-click resets get explicit reset controls
  - previews animate while pressed or active on touch devices
  - 48 dp hit areas
- **D7.** Text contrast is measured against the brightest glow behind a panel. `--faint` is for non-text marks only.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Opaque flat panels (the earlier studio look) | Cheapest to render | Loses the current design that the project chose |
| A light theme as well | Choice | Doubles design and test work; dark suits lighting work; a later option |
| A component library's default theme | Less design work | Generic; off-brand; CDN or bundle weight |

## Consequences

- RB-11 implements the specification.
- Visual regression screenshots, captured in demo mode, are part of the end-to-end suite.
- Contrast checks run in CI on the token set.
