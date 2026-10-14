# Display capture (screen sync)

Status: Accepted · Implementation: Not started

Decided in [ADR-0015](../decisions/ADR-0015-display-capture.md).

## Requirements

- **On demand:** capture runs only while an active layer uses a screen input.
- **One selected screen per source** in v1. A source lists every screen it can capture as an input. Several screens as separate inputs come later.
- **Content:** SDR. When the desktop runs in HDR mode, the output must still look right, so HDR is detected and tone-mapped.
- **Black-bar** (letterbox and pillarbox) detection.
- **Output:** a small 2D frame for the Screen effect, which produces a canvas surface.
- **Platforms, in priority order:**
  1. Windows
  2. Linux **Wayland** through the desktop portal (KDE Plasma and GNOME)
  3. X11 later

## Where capture runs

**Capture works only inside the signed-in user's interactive session:**
- On Windows, Desktop Duplication and Windows.Graphics.Capture both fail from session 0.
- On Linux, the portal needs the graphical session's D-Bus and portal backend. System units can't use it.

So capture is done by the **capture agent** `folo-capture` (Rust), which always runs in the user session:

| Node runs as | Who starts the capture agent |
|---|---|
| Desktop-managed node (user mode) | The node, on demand |
| Windows service | The desktop app at sign-in. Later, the service may launch it into the console session (`CreateProcessAsUser`). |
| Linux system unit or Docker | Not available. Screen sync needs a user-session source. |
| Linux user unit | The node, or a user unit `app-<app-id>-capture.service` (`PartOf=`/`After=graphical-session.target`) |

The agent sends small frames over loopback to the effect engine, where the Screen effect runs.

## Windows

- **Backends, in order:**
  1. **DXGI Desktop Duplication**, using `DuplicateOutput1` with {BGRA8, R16G16B16A16_FLOAT}. A D3D11 device is created on the adapter that drives the output, with one duplication per output.
  2. **Windows.Graphics.Capture**, for cases Desktop Duplication can't handle: hybrid-GPU laptops capturing against the discrete GPU fail by design. Settings: cursor capture off, a free-threaded frame pool, borderless when the user consents (build 20348+). Otherwise the yellow border shows.
  3. GDI, as a last resort.
- **Recovery:** a state machine handles `ACCESS_LOST` (mode changes, UAC, lock screen), `DXGI_ERROR_UNSUPPORTED`, `SESSION_DISCONNECTED` and the four-processes-per-session limit, with backoff. On the secure desktop, FOLO holds or fades the last frame.
- **HDR:** detected through the output colour space (`DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`). Frames are averaged in linear FP16, then tone-mapped against the user's SDR white level (from `DISPLAYCONFIG_SDR_WHITE_LEVEL`).
- **Protected content:** Desktop Duplication's `ProtectedContentMaskedOut` flag is reported as **"protected content"** rather than black. Hardware-DRM video, for example the Netflix app or PlayReady in Edge, can't be captured.
- **Privacy setting:** newer Windows 11 builds have a "Screenshots and screen recording" setting and policy. If access is denied, FOLO reports it with a link to the setting.

## Linux (Wayland)

- **Flow:** the xdg-desktop-portal **ScreenCast** interface:
  1. `CreateSession`
  2. `SelectSources` with monitor type, hidden cursor, `persist_mode=2` and the saved `restore_token` (portal version 4 or newer)
  3. `Start`, with the FOLO window as parent for the consent dialog
  4. `OpenPipeWireRemote`, to consume the PipeWire stream
- **Tokens** are single-use: save the new one after every start. Restoring fails, and the dialog appears again, when the monitor set changes (for example a TV turned off and on). FOLO shows a notification before re-prompting.
- **Stable app id:** the unit name `app-<app-id>-capture.service`, or registration with the portal.
- **Frames:** DMA-BUF (imported into EGL or Vulkan for GPU downscaling), with shared memory as a fallback. Compositors don't downscale monitor streams.
- **KDE pre-authorization and KWin's privileged screencast protocol** are optional power-user paths, not the default.

## Downscaling

- **Always downscale on the GPU before reading back.** A 4K frame is 33 MB; mip level 3 of 3840×2160 is 480×270 (518 KB).
- Methods: a mip chain (`GenerateMips`, then copy the chosen level to a staging texture), or a box-filter shader with tone mapping in the same pass.
- The agent delivers at most 480×270. The Screen effect produces the zone's requested canvas (default 64×36, at most 128×72 on the network).

## Black-bar detection

- A port of Hyperion.NG's detector, run on the downscaled frame:
  - **Black pixel:** R, G and B all below `ceil(threshold × 255)`; default threshold 5%.
  - **Modes:** default (samples at 1/3 and 2/3 of the frame plus the centre, on all sides), letterbox (top and bottom only, ignoring subtitle areas), OSD (finds the horizontal border first, to avoid overlays).
  - **Hysteresis is time-based** rather than frame-based, because capture rates vary. A new border is adopted after a stable period. "Unknown", for example a black scene, is adopted only after a long period, and a few inconsistent frames are tolerated.
  - A blur margin absorbs scaling blur.
- Presets: Auto, Off, Force 21:9, Force 4:3. Video and game profiles: game ignores screen edges (HUD) and adopts changes faster.

## Latency and privacy

- **Latency targets:** about 50 ms or less glass-to-light for games, 100 ms or less for films. Capture costs about one frame; smoothing is usually the largest term.
- **Privacy:**
  - Capture runs on demand only, with FOLO's own "screen sync active" indicator in the tray and the UI.
  - Full frames are never stored or sent. Only the zone's low-resolution surface leaves the machine.

## Testing

- Capture sits behind an interface with synthetic and replay inputs (recorded frame sequences).
- **Golden tests** for black bars: letterbox, pillarbox, 21:9, subtitles in the bars, logos, fade to black, OSD, noise near the threshold.
- **Downscale and tone-map shaders** run on Windows' WARP software device with fixtures.
- **Linux:** a fake PipeWire screencast node in a container (`videotestsrc ! pipewiresink`). Optional end-to-end runs use a headless compositor. The consent path is tested manually.

## Libraries

| Platform | Libraries |
|---|---|
| Windows | `windows-rs` directly. The `windows-capture` crate (2.0.x) is acceptable if it exposes the raw texture for GPU downscaling. Python libraries (`dxcam`, `windows-capture` for Python, `mss`) read back full frames and are for prototypes only. |
| Linux | `ashpd` (0.13.x) for the portal, `pipewire` (0.10.x, pre-1.0) for the stream |

## Sources

- **Microsoft Learn:**
  - [Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)
  - [DuplicateOutput1](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_5/nf-dxgi1_5-idxgioutput5-duplicateoutput1)
  - [Hybrid-GPU errors](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/error-when-dda-capable-app-is-against-gpu)
  - [Screen capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)
  - [IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired)
  - [HDR](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range)
  - [Interactive services](https://learn.microsoft.com/en-us/windows/win32/services/interactive-services)
- **Linux:**
  - [xdg-desktop-portal ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
  - [xdg-desktop-portal-kde screencast](https://invent.kde.org/plasma/xdg-desktop-portal-kde/-/blob/master/src/screencast.cpp)
  - [systemd desktop environments](https://systemd.io/DESKTOP_ENVIRONMENTS/)
  - [PipeWire DMA-BUF](https://docs.pipewire.org/1.4/page_dma_buf.html)
- **Reference implementations:**
  - [Hyperion.NG black border](https://github.com/hyperion-project/hyperion.ng/tree/master/libsrc/blackborder)
  - [HyperHDR DX grabber](https://github.com/awawa-dev/HyperHDR/blob/master/sources/grabber/windows/DX/DxGrabber.cpp)
  - [Sunshine service](https://github.com/LizardByte/Sunshine/blob/master/tools/sunshinesvc.cpp)
