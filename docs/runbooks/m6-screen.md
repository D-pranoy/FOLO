# M6 Screen

Status: Accepted · Implementation: Not started

## RB-14: Display capture and Screen effect

**Goal:** on-demand screen sync on Windows and Linux Wayland (KDE and GNOME).
**Depends on:** RB-04, RB-06.

1. **Capture crate** (`folo-capture-core`):
   - a `CaptureBackend` trait: list screens, start, frames, stop
   - synthetic and replay backends for tests
2. **Windows backend:**
   - Desktop Duplication through `windows-rs`: `DuplicateOutput1` with BGRA8 and R16G16B16A16_FLOAT; a device on the output's adapter
   - Windows.Graphics.Capture fallback: cursor off, free-threaded frame pool, borderless with consent on build 20348+
   - GDI last
   - recovery state machine: `ACCESS_LOST`, `UNSUPPORTED`, `SESSION_DISCONNECTED`, the process limit, secure desktop
3. **GPU downscale:** a mip chain or box-filter shader to at most 480×270 before readback. HDR detection through the output colour space, with tone mapping in linear light against the SDR white level.
4. **Linux backend:**
   - `ashpd` ScreenCast: monitor type, hidden cursor, `persist_mode=2`, restore tokens saved after every start (portal version 4 or newer required)
   - `pipewire` stream with DMA-BUF import into EGL or Vulkan for GPU downscaling, and a shared-memory fallback
   - re-prompts shown as notifications
5. **Black-bar detection:** a port of Hyperion.NG's default, letterbox and OSD modes; time-based hysteresis; blur margin; presets (Auto, Off, 21:9, 4:3); video and game profiles. The protected-content flag is reported.
6. **Agent process** (`folo-capture`):
   - runs in the user session and starts on demand when an active layer uses a screen input
   - sends frames and metadata (bars, protected content, HDR) to `folo-source` over loopback
   - started by the node (user mode) or the desktop app (service mode)
   - Linux user unit `app-io.github.d-pranoy.folo-capture.service` with `PartOf=`/`After=graphical-session.target`
7. **Screen effect** (Python): consumes frames, then crop, colour and saturation, smoothing, edge/area modes and profiles, producing a canvas at the zone's requested resolution.
8. **UX:**
   - the screen-sync wizard: pick the screen, OS consent, black-bar preview, start at sign-in
   - a "screen sync active" indicator in the tray and UI
   - a protected-content banner
   - the privacy-setting hint on Windows
9. **Tests:**
   - black-bar golden tests (letterbox, pillarbox, 21:9, subtitles in the bars, logos, fade to black, OSD, noise near the threshold)
   - shader tests on WARP with fixtures
   - a fake PipeWire node in a container
   - replay-driven end-to-end runs through the Screen effect and projection
10. **Live verification** on Windows (SDR and HDR desktop), KDE Plasma (Arch-based and Ubuntu) and GNOME, per the [testing policy](../process/testing-policy.md).

**Done when:** the replay and golden suites are green and live verification has passed on every Tier 1 platform.
