# Scope of version 1

Status: Accepted

This document defines what version 1 delivers. Later work is listed in the [roadmap](roadmap.md).

## In scope

### Foundation
- Documentation as the source of truth, ADRs, run books and process docs.
- Engineering baseline:
  - Python: uv, ruff, pyright, pytest/hypothesis.
  - Rust: cargo fmt, clippy, cargo-deny, cargo-audit.
  - UI: TypeScript strict, ESLint, Prettier, vitest, Playwright.
  - pre-commit and CI.
- Contracts first: JSON Schema, OpenAPI and AsyncAPI as the single source, with generated types and conformance tests ([ADR-0004](decisions/ADR-0004-implementation-languages-and-contracts.md)).

### Runtime
- **Node daemon** (Rust): one binary with a controller role and a source role.
  - Settings layering, supervision, status, identity and pairing.
  - The controller runs a local source alongside itself by default ([ADR-0003](decisions/ADR-0003-system-roles.md)).
- **Source effect engine** (Python): restructured from the current engine.
  - Inputs, effects, surfaces.
  - All existing effect modes with byte-exact parity ([ADR-0005](decisions/ADR-0005-pipeline-and-data-model.md)).
- **Studio features carried over** ([ADR-0028](decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)):
  - **Audio input:** output loopback, inputs, only one app / everything except one app (Windows), default-output following, stats and meters, gain and auto level.
  - **Beat tracking** with the Beat trigger and beat-based content.
  - **Now playing:** Windows media sessions and Linux MPRIS; album colours; player controls.
  - **Audio-reactive modes and the ambient light** per zone.
  - **Efficiency:** quiet-window skipping; providers sending only changed frames with keepalives.
- **Display capture agent** (Rust): on-demand capture of one selected screen per source on Windows and on Linux Wayland (KDE and GNOME through the desktop portal).
  - Black-bar detection.
  - HDR desktops handled with a basic tone map ([ADR-0015](decisions/ADR-0015-display-capture.md)).
- **Spatial model**:
  - Several zones.
  - Point, line and grid targets.
  - Strip and canvas surfaces.
  - Projection, capability adapters, flash limiter ([ADR-0006](decisions/ADR-0006-spatial-model-zones-and-projection.md), [ADR-0007](decisions/ADR-0007-capabilities-adapters-and-flash-limiter.md)).

### Devices and integrations
- **Providers:** Hyperion, WLED and Philips Hue Entertainment, each with basic device administration ([ADR-0011](decisions/ADR-0011-hyperion-provider.md) to [ADR-0014](decisions/ADR-0014-device-administration-scope.md)).
- **Hue zone sync:** import from and export to Hue entertainment areas, plus FOLO-managed entertainment configurations per bridge.
- **Home Assistant readiness:** API features and a published async Python client library ([ADR-0022](decisions/ADR-0022-home-assistant-integration-strategy.md)).

### Networking and access
- **Networking:** only the controller listens. Sources connect out to it, are paired by invite or by an approved join request, and talk over Noise-encrypted links ([ADR-0008](decisions/ADR-0008-networking-pairing-and-encryption.md)).
- **Client access:** LAN-only, per-client API keys, QR or paste connect, several controllers per client ([ADR-0009](decisions/ADR-0009-client-access.md)).
- **OTA updates** from GitHub Releases: signed, approved, with rollback where the install type allows ([ADR-0020](decisions/ADR-0020-ota-updates.md)).

### UI and content
- **UI:** a full rewrite with feature parity ([UI parity checklist](specs/ui-parity-checklist.md)), in the glass visual language of the [visual design specification](specs/visual-design.md) ([ADR-0029](decisions/ADR-0029-visual-design-language.md)).
  - Hue-style zone setup.
  - First-run and task wizards.
  - Touch-first, responsive layouts.
  - Served by the controller and shown in the desktop app or any LAN browser ([ADR-0016](decisions/ADR-0016-user-interface.md)).
- **Content packs (data):** the built-in library moves into packs; import, export, and distribution to sources ([ADR-0021](decisions/ADR-0021-content-packs.md)).

### Delivery
- **Desktop app** (Tauri v2):
  - tray icon, and a taskbar button while the window is open
  - opt-in start at sign-in
  - host list and switching
  - manages the local node ([ADR-0017](decisions/ADR-0017-desktop-app-tray-autostart-and-service.md))
- **Packaging** ([ADR-0018](decisions/ADR-0018-packaging-and-supported-platforms.md)):
  - Windows NSIS installer that asks about the service and start at sign-in, a Windows portable zip, headless daemon archives.
  - Linux deb, rpm, AppImage, and an Arch (AUR) package.
  - Docker controller image.
- **GitHub Actions:** CI, builds of every format, releases, checksums and provenance attestations ([ADR-0019](decisions/ADR-0019-ci-cd-versioning-and-releases.md)).

## Out of scope for version 1

| Item | Status |
|---|---|
| Projection mapping (display targets, warping, GPU projection) | Groundwork only ([ADR-0025](decisions/ADR-0025-projection-mapping-groundwork.md)) |
| Several screens per source | Later; v1 captures one selected screen per source |
| Per-app audio capture on Linux | Later (PipeWire stream targets); v1 Linux captures outputs and inputs |
| Now playing while the node runs as a Windows service | Later (media integration in the user-session agent); v1 reports it as unavailable |
| A light theme | Later; v1 is dark only ([ADR-0029](decisions/ADR-0029-visual-design-language.md)) |
| X11 display capture | Later; Wayland first |
| Home Assistant integration (HACS, HA app, core submission) | After v1, phased |
| Recipe/expression effects, community pack index, code plugins | Later |
| Hue lamps outside entertainment areas (REST, slow) | Later |
| More providers (OpenRGB, LIFX, Nanoleaf, Govee, Twinkly, Art-Net/sACN/DMX, MQTT) | Later; the provider interface allows them |
| HDMI capture-card input | Later |
| Android client | Later; the UI and API are designed for it now |
| MSI installer, Windows on ARM, paid code signing | Later |
| Relaying audio between machines | Excluded ([ADR-0003](decisions/ADR-0003-system-roles.md)) |
| Access from outside the LAN, cloud services, telemetry | Excluded |
| macOS | Excluded |
| deb/rpm repositories, self-updating containers | Excluded; Docker and system packages are updated by their own tools |
