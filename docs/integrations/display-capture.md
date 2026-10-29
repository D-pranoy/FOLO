# Display capture (screen sync)

Status: Accepted · Implementation: Partial (the core crate and the agent scaffold; see [RB-14](../runbooks/m6-screen.md))

Decided in [ADR-0015](../decisions/ADR-0015-display-capture.md) and [ADR-0037](../decisions/ADR-0037-display-capture-agent.md).

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
| Windows service | The desktop app at sign-in. Later, the service may launch it into the console session (`CreateProcessAsUser`). How it then gets its launch token is decided with the service (RB-19, RB-20). |
| Linux system unit or Docker | Not available. Screen sync needs a user-session source. |
| Linux user unit | The node. (A user unit `app-<app-id>-capture.service` that systemd starts is reserved with service mode: it can't get the node's launch token until RB-19 and RB-20 decide how.) |

The agent sends small frames to its node over loopback, and the node hands them on to the effect engine, where the Screen effect runs.

## The agent and its link

Decided in [ADR-0037](../decisions/ADR-0037-display-capture-agent.md); the messages and the frame are in the [source link](../specs/source-link.md#the-capture-link).

- **Link:** the agent dials its node at `ws://127.0.0.1:<port>/api/v1/capture-link`. Its launch token and the link's URL are in its environment (`FOLO_CAPTURE_TOKEN`, `FOLO_CAPTURE_LINK`), never on its command line. The node forwards the frames unchanged to the engine over the local source link.
- **Lifecycle:** the node starts the agent while capture is **wanted**: a screen input is enabled and an enabled Screen layer is in the active scene of one of the source's surfaces. It then asks for a capture (`capture.start`) and, when capture isn't wanted any more, stops it and the agent.
- **Health:** the agent reports its state (`off`, `starting`, `waiting_consent`, `capturing`, `paused` or `error`) at least once a second. The node restarts a silent agent, and the agent exits when its node has been gone for 30 s.
- **Screens:** on Windows a screen is identified by its monitor's device path. On Linux there is one entry, `portal`: the portal names no screens before the user chooses one. `folo-capture screens --json` lists them without capturing, for the node to use when no agent is linked.

## Frames from the agent

- **Format:** `rgb8`, sRGB-encoded, in the `FOLC` frame of the [source link](../specs/source-link.md#screen-frames), never compressed. Frames are never wider than 480 or higher than 270.
- **Size:** at most 480 wide and 270 high. The node asks for twice the largest canvas among the surfaces with a Screen layer (at least 64×36, at most 480×270). The agent delivers the largest frame of the screen's aspect ratio that fits.
- **Rate:** the highest rate those surfaces publish at, at most 60 Hz. When nothing on the screen changes, the last picture is sent again after 0.25 s, as a new frame with the send time and the next sequence number.
- **Clock:** timestamps are microseconds on the operating system's monotonic clock (`QueryPerformanceCounter`, `CLOCK_MONOTONIC`), the clock the engine stamps its own frames with.
- **Flags:** `protected` (protected content is on screen), `hdr` (the desktop is HDR, and the frame was tone-mapped) and `held` (the agent can't capture right now, and sends its last picture again).
- **Bars:** the black bars found, in cells, in every frame's header. The picture isn't cropped.

## Windows

- **Backends, in order:**
  1. **DXGI Desktop Duplication**, using `DuplicateOutput1` with {BGRA8, R16G16B16A16_FLOAT}. A D3D11 device is created on the adapter that drives the output, with one duplication per output.
  2. **Windows.Graphics.Capture**, for cases Desktop Duplication can't handle: hybrid-GPU laptops capturing against the discrete GPU fail by design. Settings: cursor capture off, a free-threaded frame pool, borderless when the user consents (build 20348+). Otherwise the yellow border shows.
  3. GDI, as a last resort.
- **Recovery:** a state machine handles `ACCESS_LOST` (mode changes, UAC, lock screen), `DXGI_ERROR_UNSUPPORTED`, `SESSION_DISCONNECTED` and the four-processes-per-session limit, with backoff. On the secure desktop, the agent is `paused` and holds the last frame (`held`); the Screen effect keeps showing it, the screen's activity ends, and the controller's ambient light takes over.
- **HDR:** detected through the output colour space (`DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`). Frames are averaged in linear FP16, then tone-mapped against the user's SDR white level (from `DISPLAYCONFIG_SDR_WHITE_LEVEL`).
- **Shaders:** the downscale and tone-mapping shaders are HLSL embedded in the agent and compiled at run time with the in-box `d3dcompiler_47`.
- **Protected content:** Desktop Duplication's `ProtectedContentMaskedOut` flag is reported as **"protected content"** (the `protected` flag) rather than black. Hardware-DRM video, for example the Netflix app or PlayReady in Edge, can't be captured.
- **Privacy setting:** newer Windows 11 builds have a "Screenshots and screen recording" setting and policy. If access is denied, the agent's status says so with a link to the setting (`settings_uri`).

## Linux (Wayland)

- **Flow:** the xdg-desktop-portal **ScreenCast** interface:
  1. `CreateSession`
  2. `SelectSources` with monitor type, hidden cursor, `persist_mode=2` and the saved `restore_token` (portal version 4 or newer)
  3. `Start`, with the FOLO window as parent for the consent dialog
  4. `OpenPipeWireRemote`, to consume the PipeWire stream
- **Tokens** are single-use: save the new one after every start. The agent returns it in its status and the node keeps it in `secrets.json` (`capture.restore_token`). Restoring fails, and the dialog appears again, when the monitor set changes (for example a TV turned off and on). The agent shows a notification before re-prompting.
- **Stable app id:** the portal needs one. The agent registers it through the portal's host registry (xdg-desktop-portal 1.19 and newer). Otherwise the node starts the agent in a transient scope, `systemd-run --user --scope`, named `app-io.github.d-pranoy.folo-capture-<random>.scope`.
- **Frames:** shared-memory buffers first, downscaled on the CPU. DMA-BUF buffers imported into EGL, so the GPU can downscale them, come second. There is no Vulkan path. Compositors don't downscale monitor streams.
- **HDR:** out of scope on Linux in v1: frames are treated as SDR.
- **KDE pre-authorization and KWin's privileged screencast protocol** are optional power-user paths, not the default.

## Downscaling

- **Always downscale before the frame is copied to the CPU.** A 4K frame is 33 MB; mip level 3 of 3840×2160 is 480×270 (518 KB). On Linux's shared-memory path the compositor has already copied the frame, so it is downscaled on the CPU.
- Methods: a mip chain (`GenerateMips`, then copy the chosen level to a staging texture), or a box-filter shader with tone mapping in the same pass.
- Averages are taken in linear light, and the result is encoded to sRGB again.
- The agent delivers at most 480×270. The Screen effect produces the zone's requested canvas (default 64×36, at most 128×72 on the network).

## Black-bar detection

The agent detects the bars on the downscaled frame and reports them; the Screen effect crops them. The detector is a port of Hyperion.NG's black-border detector, which is MIT-licensed ([ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D7, [third-party notices](../../THIRD_PARTY_NOTICES)):
- **Black pixel:** R, G and B all below `ceil(threshold × 255)`; default threshold 5% (`black_bars.threshold`, 1–50%).
- **Modes** (`black_bars.mode`): default (samples at 1/3 and 2/3 of the frame plus the centre, on all sides), letterbox (top and bottom only, ignoring subtitle areas), OSD (finds the horizontal border first, to avoid overlays).
- **Hysteresis is time-based** rather than frame-based, because capture rates vary. A new border is adopted after a stable period. "Unknown", for example a black scene, is adopted only after a long period, and a few inconsistent frames are tolerated.
- A blur margin absorbs scaling blur.
- **Presets** (`black_bars.preset`): `auto`, `off`, and `21:9` and `4:3`, which force the bars of a picture of that shape.
- **Profiles** (`profile`): `video`, and `game`, which ignores the screen's edges (HUDs) and adopts changes faster.
- **Reported as:** cells in each frame's header and fractions of the frame in the agent's status. The UI's black-bar preview is drawn from the bars over the zone's surface, never from screen frames.
- Protected content is reported, not shown as black.

## The Screen effect

The `screen` effect runs in the engine. It is a **canvas effect**: it draws a 2D picture for its layer's rectangle of a canvas surface, and on a strip surface its layers draw nothing (projection, [RB-08](../runbooks/m3-controller-core.md#rb-08-zones-projection-and-adapters), maps canvases onto strips). Its parameters are in the [configuration model](../specs/config-model.md#entities-summary). For each frame it renders:
1. **The picture:** the newest frame that has neither the `held` nor the `protected` flag. The pixels of those are ignored, and the effect keeps drawing the last picture it had; a layer that never had one draws nothing.
2. **Crop:** with `bars` set to `follow`, the black bars in the frame's header are cut away; then the `margins` (shares of what remains, each rounded to the nearest cell, half up; at least one cell is left, the second side of an axis giving way first). With `ignore`, only the margins apply.
3. **Sample:** onto the layer's cells, as the layer's rectangle covers them.
   - `area`: each cell is the average of the picture's cells under it, weighted by how much of each it covers, across the columns and then down the rows, as a surface is resampled for publishing.
   - `edge`: each cell takes the colour of the picture's nearest side, spread inward. Distances are measured in cells, from the cell's centre to the rectangle's sides. The nearest side is the one with the smallest of the cell's distances to the top, bottom, left and right of the layer's rectangle (the top and bottom win ties). The colour is the average of the picture's cells in a band along that side, `edge_depth` of the picture's height deep (top, bottom) or width (left, right; a part of a cell at the band's inner end counts in part), over the same span along the side that `area` would use for the cell.
4. **Colour:** saturation around the cell's luma, then brightness, clipped to 0–255, as the `brightness` post-processing effect does.
5. **Smoothing:** each cell moves towards its new colour by `1 − e^(−dt / smoothing)` of the difference. `smoothing` 0 shows each frame as it is. The first picture appears at once, and a layer whose region changes size starts over.
6. **Blend:** the layer's `blend` applies, inside its region, as for any layer. Its `axis` doesn't apply.

The sampling of a new frame onto a layer takes about 1 ms for a 256×144 frame onto 128×72 cells, well inside the 20 ms a step has at 50 Hz; a test pins it.

**Activity and rendering:**
- Each surface with an enabled Screen layer has its own activity ([pipeline](../architecture/pipeline.md#the-executor)): active while the newest frame that isn't `held` or `protected` arrived less than 0.5 s ago (age counts from its arrival at the engine). A surface with audio layers too is active when either input is.
- While audio plays, Screen layers render with the audio steps. While it is idle, a screen frame starts a step, so they render at the capture rate.

## Latency and privacy

- **Latency targets:** about 50 ms or less glass-to-light for games, 100 ms or less for films. Capture costs about one frame; smoothing is usually the largest term.
- **Privacy:**
  - Capture runs on demand only, with FOLO's own "screen sync active" indicator in the tray and the UI.
  - Raw screen frames travel only from the agent to its node and on to its engine, on loopback. They never reach the live channel, the client API, ingest, a remote source link, a log or a file FOLO writes. The status clients see holds the state, rate, flags and bars, and nothing of the picture.
  - The zone's canvas, which the Screen effect renders from them (64×36 by default), is published like any surface and is a picture of the screen: clients holding a key can subscribe to it (`surface:<id>`, `target:<id>`) for their own preview, and on the LAN it travels over plain HTTP like the rest of the live channel. It is the only screen picture that leaves the engine.
  - Operating-system crash dumps and swap are outside FOLO's control.
  - Test fixtures and replay files are synthetic; recordings of real screens are never committed.

## Testing

- Capture sits behind an interface with synthetic and replay inputs (recorded frame sequences, committed as synthetic ones). The node is tested with a fake agent, and the agent with a fake node.
- **Golden tests** for black bars: letterbox, pillarbox, 21:9, subtitles in the bars, logos, fade to black, OSD, noise near the threshold.
- **Downscale and tone-map shaders** run on Windows' WARP software device with fixtures.
- **Linux:** on the CI runner, a real PipeWire daemon with a test producer and a fake ScreenCast portal (a service on a private `dbus-daemon`). These capture tests run in an Ubuntu 24.04 job; the workspace must still build on Ubuntu 22.04. The consent path is tested manually.
- **Privacy:** a run with marker pixels the effect can't copy (in `held` and `protected` frames, the bar rows and the margins) collects every status, levels and spectrum message, log line, the node's status, API answer and file in the temporary and data folders, and asserts neither the raw payload nor the markers are in them. The `surface:` and `target:` topics are expected to carry the canvas and nothing else.
- **End to end:** the node, the agent in replay mode and the real engine, asserting the canvas frames the engine publishes; through projection once RB-08 lands.

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
