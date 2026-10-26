# ADR-0037: The display capture agent: its link, frames and Screen effect

Status: Accepted. The orchestrator accepted it after review, on the lead maintainer's delegation.
Date: 2026-10-26
Amends: [ADR-0015](ADR-0015-display-capture.md) D5, D6 and D9 (see [Consequences](#consequences))

## Context

[ADR-0015](ADR-0015-display-capture.md) fixes what the capture agent `folo-capture` is: a Rust agent in the signed-in user's session, capturing one screen on demand, downscaling before readback, tone-mapping HDR and detecting black bars. The [display capture](../integrations/display-capture.md) document and [RB-14](../runbooks/m6-screen.md) say that it "sends frames and metadata to `folo-source` over loopback" and that the Python Screen effect turns them into a canvas. They leave open what RB-14 needs before any code:
- how the agent reaches the engine, how it is launched, and how it proves it is the node's own agent
- when the agent runs, and when it captures
- what a frame looks like on the wire, how big it is and how often it comes, and which clock stamps it
- how black bars, protected content and the secure desktop reach the Screen effect
- the Screen effect's parameters, and how a 2D effect fits the engine, whose effects draw 1D lines and whose inputs reach only the audio visualizer
- when the engine renders for screen frames, and what the activity of a screen is
- how screens are listed and named, and where the Linux restore token lives
- which Linux frame path comes first, how shaders are built, and where `unsafe` may appear
- how the pieces are tested without a real screen, and what a screen frame may never touch

Facts that shape the answer:
- Capture works only in the user's session, but the node may run elsewhere (a Windows service in session 0, a system unit). The agent therefore can't be a thread of the node or of the engine.
- A loopback port is open to every local program ([ADR-0032](ADR-0032-local-source-link.md)), and a screen is private.
- The engine renders from audio packets and publishes surfaces; it has no way for an effect to read an input other than audio.
- Existing contract tests require the effect catalog in `specs/` to match the engine's registry.

## Decision

- **D1.** **Transport:**
  - The agent dials its node at `ws://127.0.0.1:<port>/api/v1/capture-link`, with the subprotocol `folo.capture.v1`. The node accepts it on loopback only, one agent at a time, and closes any other connection.
  - The route is key-free like `/source-link`, and the node checks the Host, Origin and client address before the upgrade. Other machines get `403 loopback_only`, even with remote access on.
  - There is no Noise layer. The handshake is JSON in the first WebSocket text message each way. Every binary message after it starts with a type byte, as on the [source link](../specs/source-link.md#messages-inside-the-noise-channel): `0x01` JSON-RPC 2.0, `0x04` screen frame.
  - The node **forwards each screen frame unchanged** to its effect engine over the local source link as a `0x04` message. Both hops are newest wins: a frame replaced before it is sent is dropped.
  - Screen frames exist only on these two loopback links (D9).
- **D2.** **Launch and authentication**, as [ADR-0032](ADR-0032-local-source-link.md) D2 to D5 and [ADR-0034](ADR-0034-the-node-process.md) do for the engine:
  - The node starts the agent with `FOLO_CAPTURE_LINK` (the URL) and `FOLO_CAPTURE_TOKEN` (a random 256-bit launch token, base64url) in its environment, never on its command line.
  - The agent's handshake payload is `{version: ["1"], agent: {version, platform}, token}`. The node compares the token in constant time and answers `{version: "1", result: "ok" | "rejected"}`. The first message must arrive within 5 s, and the token is checked before the "one agent at a time" test, so a rogue local connection can't hold the slot. No message may exceed 400,000 bytes (a frame is at most 388,836).
  - The agent sends the `capture.status` notification at least once a second. After 5 s without any message from it, the node treats it as hung and restarts it with the engine's restart policy ([control plane](../architecture/control-plane.md#supervision)).
  - Losing the node: the agent reconnects with backoff from 1 s up to 60 s, each delay cut by up to 25% at random, and exits after 30 s without its node.
  - Exit status of `folo-capture`: 0 after a stop request or a restart the node asked for; 1 when the link was refused or the node lost; 2 when the environment lacks the link's URL or token.
  - **Service mode** (the desktop app starts the agent into the user's session while the node runs as a service) is reserved for RB-19 and RB-20. They decide how the agent gets its token.
- **D3.** **Lifecycle:**
  - Capture is **wanted** while an `inputs.screens[]` entry of the source is enabled **and** an enabled `screen` layer is in the active scene of a surface assigned to the source.
  - In user mode the node starts the agent when capture becomes wanted. After the handshake it sends `capture.start {screen, width, height, rate_hz, black_bars, profile, restore_token?}`. The agent answers `{}` once it accepts the request; progress is in `capture.status`.
  - The node sends `capture.start` again when `black_bars`, `profile`, `rate_hz` or the size change, and the agent applies them **in place**, without a new session or consent. A different `screen` ends the session and starts another, which may ask for consent again. `restore_token` is sent only when a capture starts from `off`: a new token from `capture.status` is stored and never sent again by itself.
  - When capture is no longer wanted the node sends `capture.stop`, then `restart {component: capture}`, which ends the agent with status 0, and kills it if it hasn't exited after 5 s.
  - On the capture link `restart` accepts only `capture`; other components get `-32602`. Methods the agent doesn't know get `-32601`, and a request it can't carry out `-32000`.
  - The agent never captures on its own: before `capture.start` and after `capture.stop` its state is `off`.
  - Where no user session is reachable (a Linux system unit, Docker, or service mode before the desktop app has started the agent), no agent runs and the screen list is empty. While capture is wanted, `SourceLive.screen` then has the state `unavailable` and a `message` saying why; while it isn't, the state is `off`.
- **D4.** **Screen frames:** a new message type, `0x04`, with the magic `FOLC` and the 36-byte header in the [source link](../specs/source-link.md#screen-frames). The payload is `width × height × 3` bytes of rgb8, row-major, never compressed.
  - Flags: bit 0 `protected`, bit 1 `hdr`, bit 2 `held` (D13).
  - The four bar fields hold the black bars found, in cells of the frame: rows at the top and bottom, columns at the left and right. The picture itself is not cropped.
  - The sequence number counts the frames the agent sent since it started, wrapping at 2³²; the first frame after a new connection counts as newer than any before it.
  - This is an additive change: the source-link contract becomes 1.4.
- **D5.** **Size and rate:**
  - The node asks for frames that fit a box of twice the width and twice the height of the largest canvas among the surfaces with an enabled screen layer, at least 64 × 36 and at most 480 × 270.
  - The agent delivers the largest frame of the screen's aspect ratio that fits the box. The size can change in the middle of a capture (a mode change); the frame header always says what it is.
  - The rate is the highest `rate_hz` those surfaces publish at, at most 60 Hz.
  - When nothing on the screen changes, the agent sends the last picture again after 0.25 s without a new one, so the frames keep arriving inside the 0.5 s staleness limit (D12). A repeated or `held` frame is a new message: it carries the time it is sent and the next sequence number.
- **D6.** **Encoding and clock:**
  - Frames are sRGB-encoded SDR in rgb8. The agent averages in linear light when it downscales and encodes the result. An HDR desktop is tone-mapped to SDR against its SDR white level, and the `hdr` flag says so.
  - Timestamps are microseconds on the operating system's monotonic clock: `QueryPerformanceCounter` on Windows, `CLOCK_MONOTONIC` on Linux. The engine's clock (`time.monotonic_ns`, Python 3.13) reads the same counter on the same machine, so the agent's and the engine's stamps compare directly.
- **D7.** **Black bars:**
  - The agent detects them on the downscaled frame, and the Screen effect applies the crop (D8). The settings are `black_bars {preset, mode, threshold}` and `profile` on each `inputs.screens[]` entry, and reach the agent in `capture.start`. The behaviour is in [display capture](../integrations/display-capture.md#black-bar-detection).
  - **The detector is a port of Hyperion.NG's black-border detector**, which is MIT-licensed, adapted to time-based hysteresis. Its copyright and licence notice are recorded in `THIRD_PARTY_NOTICES` ([ADR-0023](ADR-0023-legacy-code-and-licensing.md) D4). The port arrives with the black-bar change of RB-14; ADR-0015 D7 already names Hyperion.NG as its source.
  - The bars are reported twice: in each frame's header (cells) and in `capture.status` as fractions of the frame.
  - The UI's black-bar preview is drawn from the reported bars over the zone's surface, never from screen frames.
- **D8.** **The Screen effect** (`screen`, in the engine) takes the newest usable frame and makes a canvas for each layer's rectangle. Its parameters are in the [configuration model](../specs/config-model.md#entities-summary): `mode` (`area` or `edge`), `edge_depth`, `margins`, `brightness`, `saturation`, `smoothing` and `bars` (`follow` or `ignore`). Defaults: `area`, 0.15, 0 on every side, 1, 1, 150 ms, `follow`. The steps are in [display capture](../integrations/display-capture.md#the-screen-effect).
- **D9.** **Privacy.** Two things are kept apart: the **raw capture frames** (`FOLC`), and the **canvas** the Screen effect renders from them.
  - **Raw frames** travel only from the agent to the node and from the node to its engine, on loopback. They never reach the live channel, the client API, ingest, a remote source link, a log, a status message or a file that FOLO writes. FOLO doesn't control operating-system crash dumps or swap, and says so rather than promising otherwise.
  - **The canvas** is published like any surface (64 × 36 by default, at most 128 × 72), and is an image of the screen: the zone's `surface:<id>` and `target:<id>` topics carry it to every client that subscribes, for the user's own preview, and the controller uses it. It needs no scope beyond the client key every live subscription needs, which also means a client on the LAN can watch it over the same plain HTTP as the rest of the live channel ([security](../architecture/security.md)). The canvas is the **only** screen-derived picture that leaves the engine.
  - The status clients see (`SourceLive.screen`) holds the state, rate, flags and bars only. Nothing else reads screen pixels.
  - Tests prove it (D22); fixtures and replay files are synthetic.
- **D10.** **Canvas effects:** the effect interface gains a third kind, `CanvasEffect`, which draws a 2D picture of rows × columns for its layer's rectangle. On a strip surface its layers draw nothing; projection ([RB-08](../runbooks/m3-controller-core.md#rb-08-zones-projection-and-adapters)) maps canvases onto strips. Effect factories get a handle to the engine's inputs (`EffectInputs`, with `screen`) beside the analyzer and the random generator. The existing effects ignore it.
- **D11.** **Render trigger:** while audio packets arrive, screen layers render with the audio steps, with the newest frame at each. While audio is idle (no packet for 0.1 s), the arrival of a screen frame starts a step, so screen layers render at the capture rate instead of the idle 30 fps.
- **D12.** **Activity:** each surface with an enabled screen layer gets its own `activity {surface, active, quiet_s}` notification, without `level_db`. It is active while the newest frame that is neither `held` nor `protected` arrived less than 0.5 s ago (its age counts from its arrival at the engine, on the engine's clock); `quiet_s` counts the seconds since it last was. A surface that also has enabled audio layers is active when either input is, so while a screen is captured and unchanged, such a surface doesn't go ambient. Other surfaces keep the source-wide audio activity.
- **D13.** **Secure desktop and protected content:**
  - On the secure desktop (UAC, the lock screen), when the session disconnects and during a mode change, the agent's state is `paused` and it sends its last picture again with `held` set, as D5 says.
  - While protected content is on screen the agent sets `protected` on its frames.
  - The Screen effect ignores the pixels of `held` and `protected` frames and keeps drawing the last picture without either flag. Activity goes inactive (D12), and the controller's ambient light crossfades in.
- **D14.** **The Linux restore token** is kept in `secrets.json` as `capture.restore_token`. The node passes it in `capture.start`, the agent returns the new one in `capture.status`, and the node stores it and removes it before it shows the status to clients.
- **D15.** **Screens and listing:**
  - On Windows a screen's id is its monitor's device path. On Linux there is one entry, `portal`, whose size is 0 × 0 until the stream starts, because the portal names no screens before the user chooses one.
  - The listing is the agent's answer to `screens.list`, or, with no agent linked, the output of `folo-capture screens --json`, which lists without capturing and shows no consent dialog. The node merges it into the answer to `inputs.list`, whose `screens` the engine leaves empty, and into `GET /sources/{id}/inputs`.
- **D16.** **The Linux app id:** the portal needs a stable application id. The agent registers one through the portal's host registry (xdg-desktop-portal 1.19 and newer). Without it the node starts the agent in a transient scope, `systemd-run --user --scope`, named `app-io.github.d\x2dpranoy.folo\x2dcapture-<random>.scope` (the dashes of the app id are escaped, as systemd does for app scopes). The Linux change verifies on a real portal that it reads the app id back as `io.github.d-pranoy.folo-capture`.
- **D17.** **The Linux frame path:** shared-memory buffers first, downscaled on the CPU; DMA-BUF imported into EGL, for downscaling on the GPU, second. No Vulkan. Linux HDR is out of scope in v1: its frames are treated as SDR. This amends ADR-0015 D5 and D6 for Linux.
- **D18.** **Windows shaders** are HLSL embedded in the agent and compiled at run time with the in-box `d3dcompiler_47`. No shader toolchain is needed at build time and no library ships.
- **D19.** **`unsafe`** is allowed only in the FFI modules of the backend crates, as per-item `#[allow(unsafe_code)]` with a `SAFETY` comment, the pattern the node already uses. The core crate and the binary stay safe code, and the workspace's `unsafe_code = "deny"` stays.
- **D20.** **Crates:** `folo-capture-core` (types, the backend trait, errors, the frame codec, the colour and downscale reference, the synthetic and replay backends, the bar detector), `folo-capture-windows`, `folo-capture-linux` and `folo-capture` (the binary, with `run` and `screens --json`, and `--backend auto|dda|wgc|gdi|portal|synthetic|replay:<file>`).
- **D21.** **What RB-14 delivers of the UX:** the `SourceLive.screen` status, which clients get with the `input:<source>` topic, and the API behind it. The screen-sync wizard and the protected-content banner move to RB-11, and the tray's "screen sync active" indicator to RB-19 step 3. The agent sends the re-prompt notification for a changed monitor set itself.
- **D22.** **Tests:**
  - The synthetic and replay backends, a fake node for the agent and a fake agent for the node.
  - Black-bar golden tests, and shader tests on Windows' WARP software device.
  - On Linux, on the CI runner: a real PipeWire daemon with a test producer, and a fake ScreenCast portal (a service on a private `dbus-daemon`). They run in an Ubuntu 24.04 job, and the workspace must still build on Ubuntu 22.04.
  - Privacy: a run sends frames whose raw bytes the effect can't copy to a canvas: marker pixels in the `held` and `protected` frames, in the black-bar rows and columns, and in the margins. It then collects every `status`, `levels` and `spectrum` message, every log line, the node's status, the answers of the client API and every file in the temporary and data folders, and asserts that neither the raw payload bytes nor the markers are in any of them. The `surface:` and `target:` topics are named as expected to carry the canvas, and the test asserts they carry exactly the rendered canvas and none of the markers.
  - The consent paths, Desktop Duplication on real GPUs, HDR and DRM content are verified live, under the [testing policy](../process/testing-policy.md).
- **D23.** **End to end:** a `FOLO_E2E=1` test runs the node, the agent in replay mode and the real engine, and asserts the canvas frames the engine publishes. It extends through projection when RB-08 lands.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| The engine listens and the agent dials it | One hop fewer | A second listening port and token in Python; the node wouldn't see the agent's health, or be able to supervise it |
| Shared memory between the agent and the engine | No copy | A service in session 0 needs `Global\` names and access lists; no back-pressure; two ways to be wrong on each OS |
| The agent's stdout | Private to the pair; no port | Binary framing and flow control on a pipe, and stderr already carries its logs; the node couldn't relay it |
| Compress frames, or send JSON | Smaller, or readable | Frames are at most 389 KB and loopback is free; a codec costs time at 60 Hz |
| Crop the bars in the agent | Smaller frames | `bars: ignore` and the preview couldn't see the bars; the agent would own an effect decision |
| Write the black-bar detector clean-room | No third-party notice | Hyperion.NG's detector is proven on real films and MIT-licensed, which ADR-0023 D4 allows |
| Render screen layers only at the audio rate | No change to the executor | A silent screen would render at 30 fps instead of the capture rate |
| One `activity` for the whole source | No change | A screen surface would show the audio's silence as its own |

## Consequences

- **Contracts, additive** (source-link 1.4): the `screen` effect and its parameters in `show.json`; `black_bars` and `profile` on `ScreenInput`; `capture.restore_token` in `secrets.json`; the capture link's messages, the `0x04` screen frame and `SourceLive.screen`; and `Screen`'s width and height may be 0 while unknown. The clamp corpus gains cases for them.
- **Documents:** [display capture](../integrations/display-capture.md), [overview](../architecture/overview.md), [pipeline](../architecture/pipeline.md), [control plane](../architecture/control-plane.md), [security](../architecture/security.md), [source link](../specs/source-link.md), [configuration model](../specs/config-model.md), [client API](../specs/client-api.md), [user interface](../architecture/ui.md), [RB-14](../runbooks/m6-screen.md), RB-11 ([m4](../runbooks/m4-interface.md)), RB-19 ([m8](../runbooks/m8-release.md)) and the [testing policy](../process/testing-policy.md).
- **Code:** the engine registers a `screen` effect that draws nothing, so the catalog and the registry agree. The rest follows in small changes: the capture crates, the node's capture supervision and `/capture-link`, the engine's screen input and canvas effects, and the end-to-end test.
- **Verified later, live:** Windows 11 in SDR and HDR, KDE Plasma on Arch and Ubuntu 24.04, and GNOME on Ubuntu 24.04. This covers the three Windows backends on real GPUs, hybrid-GPU laptops, the secure desktop, mode changes, the four-process limit, DRM content, the privacy setting, portal consent and restore tokens, DMA-BUF on real drivers, the app id and the latency.
- **Amends ADR-0015:** D5 (downscale before readback): on Linux's shared-memory path the compositor has already copied the frame, so it is downscaled on the CPU. D6 (HDR is tone-mapped): not on Linux in v1. D9 ("full frames are never stored or sent"): raw frames now travel agent to node to engine on loopback, and nowhere else (D9 here). Its other decisions stand.
- **A new third-party file:** `THIRD_PARTY_NOTICES`, with Hyperion.NG's notice.
