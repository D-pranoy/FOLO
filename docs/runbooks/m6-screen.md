# M6 Screen

Status: Accepted · Implementation: Partial (RB-14 steps 1 and 5: the core crate, the agent scaffold and the bar detector; the backends, the link and the Screen effect follow)

## RB-14: Display capture and Screen effect

**Goal:** on-demand screen sync on Windows and Linux Wayland (KDE and GNOME).
**Depends on:** RB-04, RB-06.

Decided in [ADR-0037](../decisions/ADR-0037-display-capture-agent.md): the agent's link, frames and the Screen effect. The contracts it adds (the capture link, the `0x04` screen frame, the `screen` effect's parameters, the screen input settings, `SourceLive.screen`) land with that ADR; the steps below implement them.

1. **Capture crates:**
   - `folo-capture-core`: a `CaptureBackend` trait (list screens, start, frames, stop), the errors, the `FOLC` frame codec, the colour and downscale reference, and the synthetic and replay backends for tests
   - `folo-capture-windows` and `folo-capture-linux` for the backends, and `folo-capture`, the binary (`run`, `screens --json`, `--backend`)
   - `unsafe` code only in the backend crates' FFI modules (ADR-0037 D19)
   - done so far: `folo-capture-core` (the types, the trait, the codec, the colour and sizing reference, the frame pacer, the synthetic and replay backends, the bar detector) and the `folo-capture` binary with `run`, `screens --json` and `--backend`; `run` stops at the link client, which comes with the node's side
2. **Windows backend:**
   - Desktop Duplication through `windows-rs`: `DuplicateOutput1` with BGRA8 and R16G16B16A16_FLOAT; a device on the output's adapter
   - Windows.Graphics.Capture fallback: cursor off, free-threaded frame pool, borderless with consent on build 20348+
   - GDI last
   - recovery state machine: `ACCESS_LOST`, `UNSUPPORTED`, `SESSION_DISCONNECTED`, the process limit, secure desktop
3. **GPU downscale:** a mip chain or box-filter shader to at most 480×270 before readback. HDR detection through the output colour space, with tone mapping in linear light against the SDR white level.
4. **Linux backend:**
   - `ashpd` ScreenCast: monitor type, hidden cursor, `persist_mode=2`, restore tokens saved after every start (portal version 4 or newer required)
   - `pipewire` stream with shared-memory buffers downscaled on the CPU first, then DMA-BUF import into EGL for GPU downscaling; no Vulkan, and no HDR on Linux in v1
   - the app id through the portal's host registry, or a transient scope named `app-io.github.d-pranoy.folo-capture-<random>.scope`
   - the restore token returned in the agent's status and kept by the node in `secrets.json`
   - re-prompts shown as notifications, sent by the agent
5. **Black-bar detection:** a port of Hyperion.NG's black-border detector (MIT; its notice is in `THIRD_PARTY_NOTICES`): the default, letterbox and OSD modes; time-based hysteresis; blur margin; presets (Auto, Off, 21:9, 4:3); video and game profiles. The protected-content flag is reported.
   - done so far: the detector in `folo-capture-core`'s `bars` (the three modes, the threshold, time-based hysteresis with the periods in [display capture](../integrations/display-capture.md#black-bar-detection), the blur margin, the presets and both profiles) with the golden tests of step 9; the agent's run loop will call it before the pacer, and backends set the protected flag
6. **Agent process** (`folo-capture`) and its link:
   - runs in the user session and starts on demand when an enabled screen input and an enabled Screen layer are in an active scene
   - dials the node's capture link with its launch token, sends `capture.status` at least once a second, and sends frames (`FOLC`) with bars, protected, HDR and held flags
   - the node's side: starting and stopping the agent, `capture.start` and `capture.stop`, forwarding frames to the engine, the screen listing in `inputs.list`, `SourceLive.screen`, the restore token in `secrets.json`
   - started by the node (user mode) or the desktop app (service mode, RB-19 and RB-20)
   - a Linux user unit `app-io.github.d-pranoy.folo-capture.service` is reserved with service mode (RB-19, RB-20): the node's launch token can't reach it otherwise
7. **Screen effect** (Python): a canvas effect that consumes frames, then crop (black bars and margins), edge or area sampling, colour and saturation, smoothing, producing a canvas at the zone's requested resolution. This brings the engine's screen input, `CanvasEffect`, the inputs handle for effect factories, the render trigger and the per-surface screen activity ([pipeline](../architecture/pipeline.md)).
8. **UX in this milestone:**
   - the `SourceLive.screen` status and the API behind it (state, rate, protected, HDR, bars, the privacy-setting link)
   - the agent sends its own re-prompt notifications
   - the screen-sync wizard and the protected-content banner are built with the interface (RB-11), and the "screen sync active" indicator in the tray with the desktop app (RB-19 step 3)
   - the black-bar preview is drawn from the reported bars over the zone's surface, never from screen frames
9. **Tests:**
   - black-bar golden tests (letterbox, pillarbox, 21:9, subtitles in the bars, logos, fade to black, OSD, noise near the threshold); done, in `crates/folo-capture-core/tests/bars.rs`
   - shader tests on WARP with fixtures
   - on Linux, on the CI runner: a real PipeWire daemon with a test producer and a fake ScreenCast portal on a private `dbus-daemon`, in an Ubuntu 24.04 job (the workspace must still build on 22.04)
   - privacy: marker pixels in the frames of a run must be in no live message, log line or temporary file
   - a replay-driven end-to-end run (node, agent in replay mode, the real engine) asserting the published canvas frames, extended through projection when RB-08 lands
10. **Live verification** on Windows (SDR and HDR desktop), KDE Plasma (Arch-based and Ubuntu) and GNOME, per the [testing policy](../process/testing-policy.md).

**Done when:** the replay and golden suites are green and live verification has passed on every Tier 1 platform.
