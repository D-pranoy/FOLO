# M4 Interface

Status: Accepted · Implementation: Not started

## RB-11: User interface

**Goal:** the new UI with full parity, Hue-style zone setup, wizards and touch-first layouts.
**Depends on:** RB-07, RB-08, RB-09.

1. **Scaffold `ui/`:** Vite, Svelte 5, TypeScript strict, ESLint (flat config) and Prettier, vitest, Playwright; no CDN; system fonts. The build output is served by the controller.
2. **API layer:**
   - generated types (RB-02) and a fetch client with authentication
   - a live WebSocket client (backoff, topics, binary decoding)
   - stores with optimistic edits, keeping the edit-sequence, revision and server-id rules exactly ([parity checklist §3](../specs/ui-parity-checklist.md#3-edit-flow-and-live-data))
3. **Pure logic first, with unit tests** comparing outputs against the current functions on recorded inputs:
   - `enforce` and `dbLimits`
   - lane packing (`computeLanes`, `settleLanes`, `pinLanes`)
   - kind detection and placement (`addKind`, `kindOf`, `freeLedRange`)
   - perceptual brightness, colour-range membership, preset matching, auto split
4. **Canvas components:**
   - analyzer, strip with glow, track, zone map
   - every pointer rule in the checklist; touch tolerances; HiDPI
   - performance: gradients cached, lanes recomputed only when the revision changes, no drawing while hidden
5. **Parity panels:**
   - top bar, brightness, effects menu (library), segment list
   - **schema-driven inspector**, palette editor, colours by range, reactive presets, settings
   - collapsible cards (same storage keys), toasts with Undo
   - fix every defect marked "Fix"; add Undo to every destructive action
6. **Navigation:** Home, Zones, Scenes (with Edit), Sources, Devices, System; the Advanced switch.
7. **Zone setup** per [user interface](../architecture/ui.md#zone-setup):
   - type, add lights with capability badges
   - placement map: drag, draw, grids, height snaps, imported positions
   - the live position test pattern (after confirmation); identify and sweeps; quick fixes
8. **Wizards:** first run for controller, source and client; the add-WLED, pair-Hue, add-source, create-zone, add-device and enable-screen-sync tasks. One decision per screen; skippable and resumable.
9. **Responsive and touch:**
   - breakpoints at 600 and 1024 px; bottom navigation and bottom sheets on phones
   - 48 dp targets; no hover-only controls; long-press menus; pinch and pan; steppers
10. **Accessibility:** keyboard paths for canvas edits, focus management, `aria-controls`, contrast fixes.
11. **Hosts:** the in-UI host switcher for browser clients, which navigates to other controllers' UIs. Hosts are added by scanning or pasting a connect code.
12. **End-to-end tests:** the Playwright parity suite, mirroring every checklist item, against a node in demo mode (synthetic audio, fake providers), in Chromium and WebKit, at phone, tablet and desktop sizes with touch emulation.
13. **Retire the legacy UI and server:** remove `web/`, `studio.py` and the legacy API; update `AGENTS.md` and `README.md`.

**Done when:** every parity checklist item is checked by a test or a recorded review, and the end-to-end suite is green.
