# Roadmap

Status: Accepted

The roadmap gives order and dependencies only. It contains no time estimates. Each milestone ends in a releasable state with green CI. Milestones list their [run books](runbooks/README.md).

## Version 1

```
M1 Foundation ──► M2 Engine ──► M3 Controller core ──► M4 Interface ──► M5 Devices ──► M6 Screen ──► M7 Network ──► M8 Release
       └──────── packaging and CI spikes start in M1 and grow with every milestone ────────────────────────────────────┘
```

| Milestone | Run books | Outcome | Exit criteria |
|---|---|---|---|
| **M1 Foundation** | RB-00, RB-01, RB-02 | Documentation, governance, toolchains, contracts and a safety net around today's behaviour | Docs and ADRs merged; CI green on Windows and Linux; golden frames committed; contracts generate types for Python, Rust and TypeScript; frozen test builds run |
| **M2 Engine** | RB-03, RB-04, RB-05 | The Python source worker: audio inputs (including app capture, gain, auto level), beat tracking, now playing, effects, surfaces, publishing; v1 config migration | Migrated configs render byte-identical to the golden frames; beat-tracker goldens pass |
| **M3 Controller core** | RB-06, RB-07, RB-08, RB-09, RB-10 | The Rust node: settings, store, API, zones, projection, adapters, reactive modes and the ambient light, content library, Hyperion provider, local source supervision | A migrated setup drives a fake Hyperion byte-identically (ambient fades within ±1) through the new controller; contract conformance tests pass |
| **M4 Interface** | RB-11 | The new UI: feature parity, glass visual design, zone editor, wizards, touch-first layouts | Parity checklist complete; end-to-end and visual regression tests green at phone, tablet and desktop sizes |
| **M5 Devices** | RB-12, RB-13 | WLED and Hue providers, Hue zone sync and managed configurations, device administration | Fake-device suites green; live verification passed per the [testing policy](process/testing-policy.md) |
| **M6 Screen** | RB-14 | Display capture agent and Screen effect on Windows and Linux Wayland | Replay and golden tests green; live verification passed on Windows, KDE and GNOME |
| **M7 Network** | RB-15, RB-16, RB-17, RB-18 | Remote clients, source pairing and links, OTA updates, Home Assistant readiness | Multi-node integration test green; update dry-runs pass for every install type; client library published |
| **M8 Release** | RB-19, RB-20, RB-21, RB-22, RB-23, RB-24 | Desktop app, Windows service and installer, Linux packages including Arch, Docker image, release pipeline, complete documentation | A release tag produces every artifact with checksums and attestations; install, upgrade and uninstall matrices pass |

## Version 1.x

- **Home Assistant:**
  - a HACS integration with a WLED-style entity model and approval-based pairing
  - Home Assistant context as effect inputs
  - a Home Assistant app (formerly add-on) for hosting the controller
- **More screens and capture:** several screens per source as separate inputs; X11 capture; HDMI capture-card input.
- **Audio and media:** per-app audio capture on Linux (PipeWire); now playing in Windows service mode through the user-session agent.
- **Appearance:** a light theme.
- **More providers:** OpenRGB, LIFX, Nanoleaf, Govee LAN, Twinkly, Art-Net/sACN/DMX, an MQTT bridge, Hue lamps outside entertainment areas.
- **Platforms:** MSI installer, Windows on ARM, signed Windows releases.

## Version 2

- Android client (Tauri v2 mobile).
- Recipe and expression effects, a community pack index, sandboxed WASM effect plugins.
- Projection mapping: display targets, warping, GPU projection, camera-assisted calibration.
- Home Assistant core integration.
- Cascading controllers (a controller forwarding to another).

## Ordering rules

- No milestone starts its device-facing work before its fake-device tests exist.
- The legacy studio stays usable on `main`, and `main` is frozen: it takes no merges and no fixes. The rebuild lands on the `v1` branch ([ADR-0030](decisions/ADR-0030-rebuild-on-a-v1-branch.md)).
- **Switch-over:** when M4 meets its exit criteria (M3's server and M4's UI have replaced the studio), a maintainer fast-forwards `main` to `v1` and makes `main` the default branch again ([workflow](process/workflow.md#switch-over)).
- Breaking changes to contracts require an ADR and a version bump of the affected contract.
