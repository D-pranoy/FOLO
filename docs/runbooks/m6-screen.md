# M6 Screen

Status: Accepted · Implementation: Partial (RB-14 steps 1 to 4: the core crate, the agent scaffold, the Windows backend and the Linux portal backend without its DMA-BUF path; step 5: the bar detector; step 6: the capture link, the agent's link client and the node's side; step 7: the engine's screen input, `CanvasEffect`, the Screen effect, the render trigger and the per-surface screen activity; step 9 (the black-bar goldens, the shader tests, the Linux portal tests and the replay end-to-end part); the node's launch of the Linux agent, the agent's use of the bar detector and the live verification follow)

## RB-14: Display capture and Screen effect

**Goal:** on-demand screen sync on Windows and Linux Wayland (KDE and GNOME).
**Depends on:** RB-04, RB-06.

Decided in [ADR-0037](../decisions/ADR-0037-display-capture-agent.md): the agent's link, frames and the Screen effect. The contracts it adds (the capture link, the `0x04` screen frame, the `screen` effect's parameters, the screen input settings, `SourceLive.screen`) land with that ADR; the steps below implement them.

1. **Capture crates:**
   - `folo-capture-core`: a `CaptureBackend` trait (list screens, start, frames, stop), the errors, the `FOLC` frame codec, the colour and downscale reference, and the synthetic and replay backends for tests
   - `folo-capture-windows` and `folo-capture-linux` for the backends, and `folo-capture`, the binary (`run`, `screens --json`, `--backend`)
   - `unsafe` code only in the backend crates' FFI modules (ADR-0037 D19)
   - done so far: `folo-capture-core` (the types, the trait, the codec, the colour and sizing reference, the frame pacer, the synthetic and replay backends, the bar detector) and the `folo-capture` binary with `run`, `screens --json` and `--backend`; `run` stops at the link client, which comes with the node's side
   - `folo-capture-linux` (step 4) is built, with no `unsafe` of its own: the bindings are `ashpd`'s and `pipewire`'s, and the monotonic clock goes through `rustix`
2. **Windows backend** (done: `folo-capture-windows`; the live checks of step 10 remain):
   - Desktop Duplication through `windows-rs`: `DuplicateOutput1` with BGRA8 and R16G16B16A16_FLOAT; a device on the output's adapter
   - Windows.Graphics.Capture fallback: cursor off, free-threaded frame pool, borderless with consent on build 20348+
   - GDI last
   - recovery state machine: `ACCESS_LOST`, `UNSUPPORTED`, `SESSION_DISCONNECTED`, the process limit, secure desktop
3. **GPU downscale** (done: a box-filter shader, in the same crate): a mip chain or box-filter shader to at most 480×270 before readback. HDR detection through the output colour space, with tone mapping in linear light against the SDR white level.
4. **Linux backend:**
   - `ashpd` ScreenCast: monitor type, hidden cursor, `persist_mode=2`, restore tokens saved after every start (portal version 4 or newer required)
   - `pipewire` stream with shared-memory buffers downscaled on the CPU first, then DMA-BUF import into EGL for GPU downscaling; no Vulkan, and no HDR on Linux in v1
   - the app id through the portal's host registry, or a transient scope named `app-io.github.d-pranoy.folo-capture-<random>.scope`
   - the restore token returned in the agent's status and kept by the node in `secrets.json`
   - re-prompts shown as notifications, sent by the agent
   - **done so far:** all of it except the DMA-BUF import into EGL, which is a documented follow-up (the backend offers no format modifiers, so compositors send shared memory). The portal backend (`PortalBackend`, `--backend portal`, `auto` on Linux) is the `portal` cargo feature, off by default because the `pipewire` crate needs PipeWire 1.0's development files and does not build against Ubuntu 22.04's 0.3.48; the details and the tests are in [display capture](../integrations/display-capture.md#linux-wayland). The node's side of the launch is open: starting the agent as `systemd-run --user --scope --unit=<folo_capture_linux::appid::transient_scope_name(random)> folo-capture run` where the portal has no host registry, with the launch token and link in that command's environment
5. **Black-bar detection:** a port of Hyperion.NG's black-border detector (MIT; its notice is in `THIRD_PARTY_NOTICES`): the default, letterbox and OSD modes; time-based hysteresis; blur margin; presets (Auto, Off, 21:9, 4:3); video and game profiles. The protected-content flag is reported.
   - done so far: the detector in `folo-capture-core`'s `bars` (the three modes, the threshold, time-based hysteresis with the periods in [display capture](../integrations/display-capture.md#black-bar-detection), the blur margin, the presets and both profiles) with the golden tests of step 9; the agent's run loop will call it before the pacer, and backends set the protected flag
6. **Agent process** (`folo-capture`) and its link:
   - runs in the user session and starts on demand when an enabled screen input and an enabled Screen layer are in an active scene
   - dials the node's capture link with its launch token, sends `capture.status` at least once a second, and sends frames (`FOLC`) with bars, protected, HDR and held flags
   - the node's side: starting and stopping the agent, `capture.start` and `capture.stop`, forwarding frames to the engine, the screen listing in `inputs.list`, `SourceLive.screen`, the restore token in `secrets.json`
   - done so far: the agent's link client (handshake, `capture.start`/`stop`/`restart`/`screens.list`, `capture.status` each second, reconnecting and giving up, the exit statuses) and, in the node, `GET /api/v1/capture-link`, starting and stopping the agent while capture is wanted, the hang check and restarts, the frames forwarded unchanged to the engine, the restore token in `secrets.json`, `SourceLive.screen` and the screens in `inputs.list`; all tested with a fake agent, a fake node and the synthetic backend. The node-level privacy test uses one marker pixel in every frame (with `protected`, `held` and bars set) and walks the node's home; separate markers for the bar rows and margins, the temporary folder and the canvas assertions come with the Screen effect (step 7). The agent follows the Windows backend's pause and privacy status and the portal's reported state and restore token, and stamps frames with `QueryPerformanceCounter` on Windows and `CLOCK_MONOTONIC` on Linux. Still to come: the node's launch of the Linux agent in its scope (D16)
   - started by the node (user mode) or the desktop app (service mode, RB-19 and RB-20)
   - a Linux user unit `app-io.github.d-pranoy.folo-capture.service` is reserved with service mode (RB-19, RB-20): the node's launch token can't reach it otherwise; the unit file is `packaging/linux/systemd/user/app-io.github.d-pranoy.folo-capture.service`, installed under the escaped name `app-io.github.d\x2dpranoy.folo\x2dcapture.service` that the portal reads the application id from, and nothing installs it yet
7. **Screen effect** (Python): a canvas effect that consumes frames, then crop (black bars and margins), edge or area sampling, colour and saturation, smoothing, producing a canvas at the zone's requested resolution. This brings the engine's screen input, `CanvasEffect`, the inputs handle for effect factories, the render trigger and the per-surface screen activity ([pipeline](../architecture/pipeline.md)).
8. **UX in this milestone:**
   - the `SourceLive.screen` status and the API behind it (state, rate, protected, HDR, bars, the privacy-setting link)
   - the agent sends its own re-prompt notifications
   - the screen-sync wizard and the protected-content banner are built with the interface (RB-11), and the "screen sync active" indicator in the tray with the desktop app (RB-19 step 3)
   - the black-bar preview is drawn from the reported bars over the zone's surface, never from screen frames
9. **Tests:**
   - black-bar golden tests (letterbox, pillarbox, 21:9, subtitles in the bars, logos, fade to black, OSD, noise near the threshold); done, in `crates/folo-capture-core/tests/bars.rs`
   - shader tests on WARP with fixtures
   - on Linux, on the CI runner: a real PipeWire daemon with a test producer and a fake ScreenCast portal on a private `dbus-daemon`, in an Ubuntu 24.04 job (the workspace must still build on 22.04). Done for the portal backend (`folo-capture-linux/tests/portal.rs`, gated by `FOLO_PIPEWIRE_TESTS`, the job `capture-linux` of `ci.yml`)
   - privacy: marker pixels in the frames of a run must be in no live message, log line or temporary file
   - a replay-driven end-to-end run (node, agent in replay mode, the real engine) asserting the published canvas frames, extended through projection when RB-08 lands
   - done so far: the replay end-to-end test (`crates/folo-node/tests/replay_e2e.rs`, `FOLO_E2E=1`, run by CI's Rust jobs): the node, the built `folo-capture` on a committed synthetic replay file (a normal, a held, a protected and a second normal frame, with markers in the held, protected, bar and margin areas) and the real engine publish, on `surface:wall`, exactly the canvases the Screen effect should draw from the normal frames (cell for cell, no tolerance), and the markers are in no log, status, answer, live message or file of the home. Still to come: the golden, shader and Linux-portal tests, and the run through projection when RB-08 lands (a follow-up of this test: it asserts `target:<id>` then)
10. **Live verification** on Windows (SDR and HDR desktop), KDE Plasma (Arch-based and Ubuntu) and GNOME, per the [testing policy](../process/testing-policy.md).

**Done when:** the replay and golden suites are green and live verification has passed on every Tier 1 platform.
