# M4 Interface

Status: Accepted · Implementation: Not started

## RB-11: User interface

**Goal:** the new UI with full parity, the glass visual design, Hue-style zone setup, wizards and touch-first layouts.
**Depends on:** RB-07, RB-08, RB-09.

1. **Scaffold `ui/`:** Vite, Svelte 5, TypeScript strict, ESLint (flat config) and Prettier, vitest, Playwright; no CDN; system fonts. The build output is served by the controller.
2. **Visual design foundation** per the [visual design specification](../specs/visual-design.md):
   - `tokens.css` (exported as JSON for the desktop app)
   - the fixed glow background
   - surface components: panel, row, well, raised, pill
   - focus ring and motion rules
   - **Reduce transparency** setting, and an `@supports` fallback without `backdrop-filter`
   - a contrast check of text tokens against the brightest glow, in CI
   - a component gallery page in demo mode for visual regression screenshots
3. **API layer:**
   - generated types (RB-02) and a fetch client with authentication
   - a live WebSocket client (backoff, topics, binary decoding; closed while the page is hidden)
   - stores with optimistic edits, keeping the edit-sequence, revision and server-id rules exactly ([parity checklist §3](../specs/ui-parity-checklist.md#3-edit-flow-and-live-data))
4. **Pure logic first, with unit tests** comparing outputs against the current functions on recorded inputs:
   - `enforce` and `dbLimits`, including the beat trigger
   - lane packing (`computeLanes`, `settleLanes`, `pinLanes`)
   - kind detection and placement (`addKind`, `kindOf`, `freeLedRange`)
   - perceptual brightness, colour-range membership, preset matching (including `beats`), auto split, the ambient "switch after" steps
5. **Rendering model:**
   - a single `invalidate()`-driven frame scheduler: no display-rate loop, no CSS animation at rest
   - offscreen caches for static layers
   - transform-only live bars
   - LED colours as a stretched pixel row ([performance rules](../specs/visual-design.md#performance-rules))
6. **Canvas components:** analyzer, strip with glow, track, zone map. Every pointer rule in the checklist; touch tolerances; HiDPI.
7. **Parity panels:**
   - top bar (status pills per provider and source, now-playing chip, tempo pill with beat dot, reactive switch, ambient indicator)
   - brightness, effects menu (library), segment list
   - **schema-driven inspector** (with Beat and "Flash every")
   - palette editor, colours by range, reactive presets (5 groups)
   - **Ambient light** (in the zone), **Music** (on Home), **Audio input** (in the source's inputs)
   - settings, collapsible cards (same storage keys), toasts with Undo
   - fix every defect marked "Fix"; add Undo to every destructive action; replace double-click resets with reset buttons
8. **Navigation:** Home, Zones, Scenes (with Edit), Sources, Devices, System; the Advanced switch.
9. **Zone setup** per [user interface](../architecture/ui.md#zone-setup):
   - type, add lights with capability badges
   - placement map: drag, draw, grids, height snaps, imported positions
   - the live position test pattern (after confirmation); identify and sweeps; quick fixes
10. **Wizards:** first run for controller, source and client; the add-WLED, pair-Hue, add-source, create-zone, add-device and enable-screen-sync tasks. One decision per screen; skippable and resumable.
    - **Screen sync** ([RB-14](m6-screen.md)): pick the screen, the operating system's consent step, a black-bar preview drawn from the reported bars over the zone's surface, and start at sign-in. The screen's state comes from the source's `screen` status, which carries no pixels.
    - **Protected content:** a banner on the zone when the status says `protected`; the Windows privacy-setting hint when the status gives a `settings_uri`.
11. **Responsive and touch:**
    - breakpoints at 600 and 1024 px; bottom navigation and glass bottom sheets on phones
    - 48 dp targets; no hover-only controls; previews while pressed or active; long-press menus; pinch and pan; steppers
12. **Accessibility:** keyboard paths for canvas edits, focus management, `aria-controls`, contrast fixes.
13. **Hosts:** the in-UI host switcher for browser clients, which navigates to other controllers' UIs. Hosts are added by scanning or pasting a connect code.
14. **End-to-end tests:**
    - the Playwright parity suite, mirroring every checklist item, against a node in demo mode (synthetic audio and songs, fake media session, fake providers)
    - in Chromium and WebKit, at phone, tablet and desktop sizes with touch emulation
    - **visual regression screenshots**
    - an idle-CPU check: no animation frames or timers at rest
15. **Retire the legacy UI and server:** remove `web/`, `studio.py` and the legacy API; update `AGENTS.md` and `README.md`.

**Done when:**
- every parity checklist item is checked by a test or a recorded review
- the end-to-end and visual regression suites are green
- the page meets the [performance budget](../architecture/ui.md#performance)
