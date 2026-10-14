# ADR-0016: User interface

Status: Accepted
Date: 2026-10-13

## Context

The current UI is a single 2,965-line JavaScript file with many subtle behaviours. All of them must be kept. The new UI must also be simple, like the Hue app's entertainment area setup, touch-first, responsive, and ready for an Android client.

## Decision

- **D1.** Svelte 5 + TypeScript (strict) + Vite. No CDN. Served by the controller and version-locked to it.
- **D2.** **Feature parity** with every item in the [UI parity checklist](../specs/ui-parity-checklist.md), with the listed defects fixed and Undo on every destructive action.
- **D3.** Navigation: Home, Zones, Scenes, Sources, Devices, System, with an Advanced switch. The current editor becomes Scenes → Edit.
- **D4.** **Hue-style zone setup** on a top-down map with a height control, a live position test, and identify and sweep tests.
- **D5.** **First-run wizards** for controller, source and client, plus task wizards. One decision per screen; every wizard can be skipped and resumed.
- **D6.** **Touch-first, responsive:** breakpoints at 600 and 1024 px, touch targets of at least 48 dp, larger canvas hit areas on coarse pointers, no hover-only controls, bottom sheets on phones. Phones can edit everything, but their layout is optimised for control.
- **D7.** **Accessibility:** a keyboard path for every edit; WCAG 2.2 AA outside the canvases.
- **D8.** The current dark theme and the Deep Night palette remain the base design.
- **D9.** The inspector is generated from effect parameter schemas.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| React + TypeScript | Biggest ecosystem | More boilerplate; render tuning needed for live panels |
| Vue 3 | Approachable | No advantage here |
| Vanilla TS modules or Lit | No framework | Rebuilding state management by hand |

## Consequences

A Playwright parity suite runs at phone, tablet and desktop sizes, in Chromium and WebKit. Details: [user interface](../architecture/ui.md).
