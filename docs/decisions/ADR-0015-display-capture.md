# ADR-0015: Display capture

Status: Accepted (D5, D6 and D9 amended by [ADR-0037](ADR-0037-display-capture-agent.md))
Date: 2026-10-13

## Context

Sources must provide on-demand display capture with black-bar detection on Windows and Linux. The research findings are in [display capture](../integrations/display-capture.md):
- Capture works only in the signed-in user's session.
- On Windows, Desktop Duplication is fast and silent but fails on hybrid GPUs against the discrete GPU; Windows.Graphics.Capture covers that case.
- HDR desktops need tone mapping.
- Hardware-DRM video captures black.
- Wayland capture goes through the desktop portal with consent and restore tokens.
- Full-resolution readback is too expensive, so downscaling must happen on the GPU.

## Decision

- **D1.** Capture is done by **`folo-capture`**, a Rust agent that always runs in the user session and starts on demand.
- **D2.** v1 captures **one selected screen per source**. A source lists every screen as an input. Several screens as separate inputs come later.
- **D3.** **Windows:** Desktop Duplication (`DuplicateOutput1`) first, Windows.Graphics.Capture as fallback (borderless with consent on build 20348+), GDI last. A recovery state machine handles mode changes and session events.
- **D4.** **Linux:** Wayland first, through the xdg-desktop-portal ScreenCast interface and PipeWire, for **KDE and GNOME**. `persist_mode=2` with rotating restore tokens. X11 comes later.
- **D5.** **GPU downscaling** happens before readback (at most 480×270). Frames are averaged in linear light.
- **D6.** **SDR content** is the target. HDR desktops are detected and tone-mapped against the SDR white level, so HDR mode never breaks the output.
- **D7.** **Black-bar detection** ports Hyperion.NG's modes with time-based hysteresis. Protected content is reported, not shown as black.
- **D8.** The Python **Screen effect** turns the agent's frames into a canvas surface, so screen and audio layers combine.
- **D9.** **Privacy:** capture runs only on demand, with FOLO's own indicator. Full frames are never stored or sent.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Capture in the Python engine (dxcam, windows-capture for Python, mss, GStreamer) | One process | Full-frame readback; awkward Wayland support; heavy packaging |
| Capture inside the service | One process | Impossible: session 0 and system units can't capture |
| The whole screen pipeline in Rust | Fewer hops | Screen layers couldn't combine with audio effects in one stack |

## Consequences

- Screen sync needs a signed-in session. Desktop installs default to user mode ([ADR-0017](ADR-0017-desktop-app-tray-autostart-and-service.md)).
- Linux packages depend on PipeWire and the portal.
