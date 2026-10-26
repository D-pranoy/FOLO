# Display capture (screen sync)

Status: Accepted · Implementation: Partial (the core crate, the capture link, the agent's link client and the Linux portal backend; see [RB-14](../runbooks/m6-screen.md))

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
  - **Several screens:** one capture runs at a time. Of several enabled `inputs.screens[]` entries the first is captured.
  - **Who asks:** the node asks only in the controller role while its own engine runs: the engine draws the Screen effect, and a source node gets its configuration from its controller (RB-16).
  - **No session:** a node launched with `--service` starts no agent. While capture is wanted its `SourceLive.screen` is `unavailable` and says why, and the screen list is empty.
  - **What clients see:** `off` while capture isn't wanted. While it is: `starting` until the agent has linked and reported; the agent's own status after that (without the restore token); `error`, with the agent's reason, when the agent refused `capture.start` (an unknown screen, say); `unavailable` while the agent can't run (it can't be started, or it exits again and again) until the node has started it again.
  - **Losing the link:** an agent that loses its node stops capturing, and captures again only when its node asks (the node asks again after the handshake). An agent in `error` stays so until the wish changes or the agent is started again.
- **Health:** the agent reports its state (`off`, `starting`, `waiting_consent`, `capturing`, `paused` or `error`) at least once a second. The node restarts a silent agent, and the agent exits when its node has been gone for 30 s.
- **Screens:** on Windows a screen is identified by its monitor's device path. On Linux there is one entry, `portal`: the portal names no screens before the user chooses one. `folo-capture screens --json` lists them without capturing, for the node to use when no agent is linked.

## Frames from the agent

- **Format:** `rgb8`, sRGB-encoded, in the `FOLC` frame of the [source link](../specs/source-link.md#screen-frames), never compressed. Frames are never wider than 480 or higher than 270.
- **Size:** at most 480 wide and 270 high. The node asks for twice the largest canvas among the surfaces with a Screen layer (at least 64×36, at most 480×270): the widest and the highest of their published sizes (canvases at most 128×72, a strip its length by 1). The agent delivers the largest frame of the screen's aspect ratio that fits.
- **Rate:** the highest rate those surfaces publish at, at most 60 Hz (today every surface is published at 60 Hz, which is what the node asks for). When nothing on the screen changes, the last picture is sent again after 0.25 s, as a new frame with the send time and the next sequence number. Every frame, new or repeated, carries the time it is sent. Repeats are not held to `rate_hz`, and a new picture waiting for the rate holds them back, so rates under about 4 Hz can't keep frames inside the engine's 0.5 s limit.
- **Clock:** timestamps are microseconds on the operating system's monotonic clock (`QueryPerformanceCounter`, `CLOCK_MONOTONIC`), the clock the engine stamps its own frames with.
- **Flags:** `protected` (protected content is on screen), `hdr` (the desktop is HDR, and the frame was tone-mapped) and `held` (the agent can't capture right now, and sends its last picture again; a picture just captured never carries it).
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
  3. `Start`
  4. `OpenPipeWireRemote`, to consume the PipeWire stream
- **The backend** is `PortalBackend` in `folo-capture-linux` (`--backend portal`, and `auto` on Linux). Its choices:
  - **One screen, `portal`:** `list_screens` never talks to the portal, so listing shows no dialog. Its size is 0 × 0 until the stream runs, then the stream's.
  - **Version:** a portal older than ScreenCast version 4 is refused with an error that says so, since it cannot keep restore tokens.
  - **No parent window:** `Start` gets no window identifier, because the agent has no window. The consent dialog is therefore not attached to the FOLO window.
  - **`start` returns at once.** The portal call runs on a thread of its own, and the state follows in `capture.status`: `starting`, then `waiting_consent` once `Start` has not answered for 1.5 s (a saved choice that still holds answers at once), then `capturing` when the stream runs.
  - **Failures stay failures:** declining the dialog, a portal that is too old, and the desktop ending the share (the user stops it from the compositor's indicator, or the compositor drops it) all end in the state `error` with the reason. The backend never starts the share again by itself, because the user may have meant to stop it; the node decides, and sends `capture.start` again.
  - **In place:** a repeated `capture.start` changes the box and the rate of the running capture and does not ask the portal anything.
- **Tokens** are single-use: save the new one after every start. The backend hands the token the portal returned over once, in its report; the agent returns it in `capture.status` and the node keeps it in `secrets.json` (`capture.restore_token`). Restoring fails, and the dialog appears again, when the monitor set changes (for example a TV turned off and on). The portal does not say that a restore failed, so the agent takes it from the timing: when a token was sent and `Start` has not answered after 1.5 s, the dialog is up, and the agent shows a desktop notification through the Notification portal ("the screens changed, the desktop is asking again which one FOLO may capture"). That is the re-prompt notification of [ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D21; it comes together with the dialog, not before it, since nothing earlier is known. A first dialog, with no token, shows no notification. The guess can be wrong: a slow but valid restore (a cold portal backend) shows the notice too, and so does every start when the portal doesn't know the app id and so keeps no tokens; the notice is not withdrawn after consent.
- **Stable app id:** the portal needs one, and the agent uses `io.github.d-pranoy.folo-capture`.
  - **Registered by the agent:** it calls the host registry (`org.freedesktop.host.portal.Registry.Register`, xdg-desktop-portal 1.19 and newer) on the connection that makes the ScreenCast calls, before them. It makes the call itself instead of through `ashpd::register_host_app`, which refuses an id with a `-` outside its last element (`d-pranoy`) before the portal can answer.
  - **Started by the node in a scope:** otherwise the node starts the agent as `systemd-run --user --scope --unit=<name> folo-capture run`, with the launch token and link in the environment of that command (never in its arguments), where `<name>` is `folo_capture_linux::appid::transient_scope_name(random)`: `app-io.github.d\x2dpranoy.folo\x2dcapture-<random>.scope`. The portal reads the id from the scope's name. The agent checks `/proc/self/cgroup` first, and does not register when it already runs in such a scope. The node can't tell whether the portal has a host registry, so on Linux it always starts the agent this way when `systemd-run` is on the `PATH` (the agent then finds itself in the scope and skips the registry). The program is the agent's path and its own arguments (`--backend ...`) come before `run`. Without `systemd-run` the node starts the agent directly, with the same environment, and logs a warning; the portal then knows the agent only if its host registry exists. `systemd-run --scope` replaces itself with the agent once the scope exists, so the node's process handling, stop and kill apply to the agent unchanged.
  - **When neither works** the capture goes on, with a warning in the status message: the portal cannot keep a restore token for an agent it doesn't know, so it asks which screen to share every time.
  - **Open:** the id was not checked against a real portal's rules. The Flatpak and `ashpd` grammar allows a `-` in the last element only, and the id (and the application id of [ADR-0026](../decisions/ADR-0026-branding-and-identifiers.md)) has one in `d-pranoy`. The live verification of RB-14 step 10 reads the id back from a real portal.
- **Frames:** shared-memory buffers first, downscaled on the CPU. DMA-BUF buffers imported into EGL, so the GPU can downscale them, come second and are not built: the stream names no format modifier, so the compositor cannot choose DMA-BUF, and a plane the agent cannot map ends the capture with an error. There is no Vulkan path. Compositors don't downscale monitor streams.
  - **Formats:** `BGRx`, `BGRA`, `RGBx`, `RGBA`, `RGB` and `BGR`, any size. Alpha is ignored.
  - **Rate:** the stream offers at most `rate_hz` frames a second, which the compositors respect, and the downscale runs at most that often. The newest buffer is never dropped, so the last picture of a still screen is always scaled. A rate change in place applies to the scaling at once, to the stream's own limit from the next capture.
  - **Threads:** the PipeWire callbacks only copy the buffer and give it back; a thread of its own downscales the copy. libpipewire may call a stream's `process` from its data thread as well as from the loop's, so nothing in the callbacks relies on one thread.
  - **Cost:** the downscale is the area mean in linear light of [Downscaling](#downscaling) (tables for the sRGB curve, no power function per sample), done in the order that reads each input pixel once, by up to four threads for a large picture. Measured in a release build on a four-core container, for a buffer scaled to 480 × 270: about 3.3 ms at 1920 × 1080, 4.7 ms at 2560 × 1440 and 8 ms at 3840 × 2160; one thread alone takes about 7 ms at 1080p and 20 ms at 4K. A 4K screen at a high rate is CPU-heavy, and the GPU path is the answer to that.
- **HDR:** out of scope on Linux in v1: frames are treated as SDR.
- **Clock:** `folo_capture_linux::MonotonicClock` reads `CLOCK_MONOTONIC` in microseconds, through `rustix`.
- **Build:** the backend is the `portal` feature (off by default), which needs the development files of PipeWire 1.0 or newer (`libpipewire-0.3-dev`, `libspa-0.2-dev`, `libclang-dev`). The `pipewire` crate does not build against Ubuntu 22.04's 0.3.48, so the Ubuntu 22.04 jobs build the workspace without the feature and a separate Ubuntu 24.04 job builds and tests it. Release builds of the Linux packages enable it where the build environment has PipeWire 1.0 (RB-21): `cargo build --release -p folo-capture --features portal`, or the packaged `folo-capture` is the one without a backend for a real screen.
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
2. **Crop:** with `bars` set to `follow`, the black bars in the frame's header are cut away; then the `margins` (shares of what remains, at least one cell is left). With `ignore`, only the margins apply.
3. **Sample:** onto the layer's cells, as the layer's rectangle covers them.
   - `area`: each cell is the average of the picture's cells under it, weighted by how much of each it covers, across the columns and then down the rows, as a surface is resampled for publishing.
   - `edge`: each cell takes the colour of the picture's nearest side, spread inward. The nearest side is the one with the smallest of the cell's distances to the top, bottom, left and right of the layer's rectangle (the top and bottom win ties). The colour is the average of the picture's cells in a band along that side, `edge_depth` of the picture's height deep (top, bottom) or width (left, right), over the same span along the side that `area` would use for the cell.
4. **Colour:** saturation around the cell's luma, then brightness, clipped to 0–255, as the `brightness` post-processing effect does.
5. **Smoothing:** each cell moves towards its new colour by `1 − e^(−dt / smoothing)` of the difference. `smoothing` 0 shows each frame as it is. The first picture appears at once, and a layer whose region changes size starts over.
6. **Blend:** the layer's `blend` applies, inside its region, as for any layer. Its `axis` doesn't apply.

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
  - **What runs:** `folo-capture-linux/tests/portal.rs` starts a private session bus, `pipewire` and `wireplumber` in a folder of its own, a test producer (the test binary run again as a separate process, offering a `BGRx` video source node with padded rows) and the fake portal (ScreenCast, the host registry and the Notification interface, with request and session objects). It checks the options sent to `SelectSources`, the token flow, the dialog and its notification, declining, an old portal, the app id registration, an ended share, stopping during the dialog and the pictures themselves.
  - **Gate:** the tests need the `portal-tests` feature and run where `dbus-daemon`, `pipewire` and `wireplumber` are installed; `FOLO_PIPEWIRE_TESTS=1` (set by CI) turns their absence into a failure instead of a skip.
  - **Not covered:** a real portal and a real compositor (consent, restore tokens, the app id, DMA-BUF), which the live verification of RB-14 step 10 does.
- **Privacy:** a run with marker pixels the effect can't copy (in `held` and `protected` frames, the bar rows and the margins) collects every status, levels and spectrum message, log line, the node's status, API answer and file in the temporary and data folders, and asserts neither the raw payload nor the markers are in them. The `surface:` and `target:` topics are expected to carry the canvas and nothing else.
- **End to end:** the node, the agent in replay mode and the real engine, asserting the canvas frames the engine publishes; through projection once RB-08 lands.

## Replay files

The replay backend (`--backend replay:<file>`) plays back recorded frames, for tests of the agent, the node and the engine. A file holds synthetic frames only; recordings of real screens are never committed. All integers are little-endian.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `FOLR` |
| 4 | 1 | File format version (1) |
| 5 | 3 | Reserved (0) |
| 8 | n | Records, until the end of the file |

A record is a `u32` length (36 to 400,000) and that many bytes of a [`FOLC` screen frame](../specs/source-link.md#screen-frames), checked as any untrusted frame is. The file holds at least one frame and at most 64 MiB.
- **Screen:** one, `replay`, named "Replay", as large as the first frame in pixels.
- **Playback:** each frame is delivered after the gap between its recorded time and the next one's (at most 1 s), and the file plays again from the first frame after the last. The gap after the last frame is that of the requested rate. The frames' sequence numbers and times are used for the gaps only: the agent numbers and stamps what it sends. Flags and bars are kept as recorded. The frame size is the recorded one: the box of `capture.start` is not applied.

The synthetic backend (`--backend synthetic`) shows one 1920 × 1080 screen, `synthetic-0`, with a deterministic test pattern (a moving gradient by default) at the size that fits the box.

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
