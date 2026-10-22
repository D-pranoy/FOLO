# User interface

Status: Accepted · Implementation: Not started

Decided in [ADR-0016](../decisions/ADR-0016-user-interface.md). Clients and hosts are covered in [ADR-0009](../decisions/ADR-0009-client-access.md).

## Principles

- **Simple by default, full granularity on demand.**
- **Touch-first and responsive.** Every screen works on phones, tablets and desktops.
- **Feature parity** with the current studio: every feature in the [UI parity checklist](../specs/ui-parity-checklist.md) is kept.
- **Version-locked.** Each controller serves its own UI.
- **Offline.** No CDN; system fonts; everything bundled.

## Technology

- Svelte 5, TypeScript (strict) and Vite. Canvases (analyzer, strip, track, zone map) stay imperative components.
- Types are generated from the OpenAPI contract ([client API](../specs/client-api.md)).
- The live WebSocket client handles reconnect with backoff, topic subscriptions and binary frame decoding.
- The edit model keeps today's conflict rules (edit sequence, revision and server id, stale detection; see the parity checklist §3).
- Tests: vitest for logic; Playwright end-to-end in Chromium and WebKit (WebKit approximates Linux WebKitGTK), at phone, tablet and desktop sizes with touch emulation.

## Navigation

| Section | Contents |
|---|---|
| **Home** | The header: status, now-playing chip, tempo pill with beat dot, audio-reactive switch (On, Auto, Off), ambient indicator, master brightness, output on/off. Then zones with their active scene and source, and the **Music** panel (now playing, player controls, album colours, beats of the bar, beat timing). |
| **Zones** | Zone list and editor: placement, surfaces, Hue sync, **reactive mode and ambient light** (colour, solid or breathe, brightness, "switch after", on pause) |
| **Scenes** | Gallery, apply to a zone, macros, **Edit** (the advanced editor) |
| **Sources** | Sources and their **inputs**: the audio input (listen to output, input or app; meters, stats, gain, auto level), screens, media session. Pairing, per-surface priority, external ingest tokens. |
| **Devices** | Providers: Hyperion, WLED, Hue. Discovery, pairing and basic administration. |
| **System** | Client devices and keys, remote access, start at sign-in, service status, updates, logs, backup and restore |

An **Advanced** switch reveals surfaces, mappings, calibration and raw configuration.

## Zone setup

Modelled on the Hue app's entertainment area setup:

1. **Type:** Music, TV/Screen, Desk, Room or Custom.
2. **Add lights:** from every provider's discovered targets, with capability badges (RGB, white, CCT; 1/7/150 elements; rate).
3. **Place:** a top-down room map with the reference object (TV, listening position, desk).
   - Drag point lights; draw lines with a direction arrow and power side; place grids with their wiring origin.
   - Set height with a slider that snaps to floor / seated / TV / ceiling.
   - Hue and Hyperion positions import automatically.
   - A **live position test pattern** runs on the real lights while placing.
4. **Test:**
   - **Identify** blinks each light.
   - **Sweeps** run left→right, front→back and floor→ceiling.
   - **Quick fixes** reverse or rotate a strip.
5. **Save**, then choose a scene.

Test patterns go to real lights only after the user confirms.

## Advanced editor (parity)

The current studio's editor becomes **Scenes → Edit** for a surface:
- the spectrum analyzer with draggable frequency and threshold boxes
- the LED/cell track with lanes
- the segment list, the inspector, palette editing, Colours by range, reactive presets

The inspector is **generated from each effect's parameter schema**, so effects that are new to the UI get controls automatically.

## Wizards

- **First run on a controller:** Welcome → Photosensitivity warning ([below](#photosensitivity)) → Role → Audio (live meter) → Find lights (discovery; the Hue link-button step) → Create zone → Pick a scene → Add a phone or source (QR) → Done.
- **First run on a source:** join a controller (scan or paste an invite, or pick a discovered controller and approve there); check inputs.
- **First run on a client:** scan or paste a controller's connect code.
- **Task wizards:** add WLED, pair Hue, add a source, create a zone, add a device, enable screen sync. The screen-sync wizard includes picking the screen, the operating system's consent step, a black-bar preview, and turning on start at sign-in.
- **Rules:** one decision per screen; large primary buttons; every wizard can be skipped and resumed. The photosensitivity warning is shown even when the rest of the first run is skipped.

## Responsive and touch rules

| Width | Layout |
|---|---|
| Under 600 px | Single column, bottom navigation, inspectors as bottom sheets |
| 600–1024 px | Two panes |
| Over 1024 px | Three columns (navigation, editor, inspector) |

- Touch targets are at least 48 dp. On coarse pointers, canvas hit tolerance grows from 6 px to 16 px and handles get bigger.
- **No hover-only controls.** Every hover affordance has a visible or long-press alternative:
  - Previews animate while pressed, or when the item is active.
  - Double-click resets (gain, beat timing) become reset buttons.
- Pinch and pan on the zone map and the track. Steppers next to sliders.
- **Phones:** everything is editable, but the phone layout is optimised for control. The advanced editor defaults to lists and sliders, and nothing requires the canvases.

## Accessibility

- A keyboard path for every edit: numeric fields and lane up/down replace canvas drags.
- Focus management when views rebuild; `aria-controls` on disclosure buttons.
- WCAG 2.2 AA contrast and target sizes outside the canvases.

## Photosensitivity

The flash limiter is off by default ([ADR-0033](../decisions/ADR-0033-flash-limiter-off-by-default.md)), so the UI makes sure people know it exists. Wherever the warning appears in full, it uses ADR-0033's text.
- **First run:** the full warning, before any light is driven, with a primary button "Turn on the flash limiter for all zones" and a secondary "Continue without it".
- **Zone settings:** the flash limiter switch, with a short warning and a link to the full text beside it while it's off.
- **Settings:** the full warning and a switch that turns the limiter on or off for all zones.
- **Effects menu:** a "Flashing" badge on built-in effects that flash quickly: the beat trigger on half beats or every beat, Strobe, Bar and similar. The badge links to the warning.

## Visual design

The [visual design specification](../specs/visual-design.md) defines the look of every screen ([ADR-0029](../decisions/ADR-0029-visual-design-language.md)):
- **Glass:** frosted, translucent panels over a fixed background of Deep Night glows.
- **Depth:** a surface hierarchy of panels, rows, wells and raised controls.
- **Single source:** design tokens defined once.
- **Quiet at rest:** no motion unless data or the user moves something.
- **Comfort settings:** a Reduce transparency setting, and dark only in v1.

## Performance

The UI's **performance rules** are normative ([visual design](../specs/visual-design.md#performance-rules)):
- draw only on invalidation (stream data, input, resize, state change)
- no CSS animation at rest
- static canvas layers cached offscreen
- live bars moved by `transform`
- hidden pages close the live stream
- no nested backdrop filters

**Budget:** no measurable CPU use at rest. While streaming, the page must stay at or below the current studio's figure (about 0.6 browser cores at 1920×1080, 144 Hz).

## Clients and hosts

- Clients keep a **host list**. Each controller can have several addresses (LAN IP, mDNS name), tried in order. One host is **active** at a time, with switching from the host menu.
- The desktop app and the future Android app show each controller's own served UI, plus a small built-in **Hosts** screen for adding, listing and switching.
- Adding a host: scan or paste a connect code ([pairing and connect](../specs/pairing-and-connect.md#client-connect-code)).
- **Android (later):** a Tauri v2 Android build of the desktop app's shell. Groundwork in v1:
  - responsive, touch-first UI
  - per-client keys
  - the connect-code format
  - CORS rules admitting app origins
  - cleartext LAN traffic allowed in the app
