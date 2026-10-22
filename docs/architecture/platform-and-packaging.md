# Platform and packaging

Status: Accepted · Implementation: Not started

Decided in [ADR-0017](../decisions/ADR-0017-desktop-app-tray-autostart-and-service.md) and [ADR-0018](../decisions/ADR-0018-packaging-and-supported-platforms.md). Release automation is in [ADR-0019](../decisions/ADR-0019-ci-cd-versioning-and-releases.md).

## Supported platforms

| Tier | Platforms | Meaning |
|---|---|---|
| **Tier 1** | Windows 11 x64; Ubuntu 24.04 LTS (KDE and GNOME, Wayland); Arch Linux and derivatives such as CachyOS (KDE Plasma, Wayland) | CI-tested and release-blocking |
| **Tier 2** | Windows 10 22H2; Ubuntu 22.04; Debian 12+; current Fedora; X11 sessions (no screen capture in v1) | Built and smoke-tested; best effort |
| **Docker** | linux/amd64, linux/arm64 | Controller image |
| **Later** | Windows on ARM, Android client | See the [roadmap](../roadmap.md) |

## Desktop app (Tauri v2)

- **Tray icon** whenever the app runs. Its icon shows status (ok, warning, off, updating). The menu offers Open, Output on/off, Start at sign-in, Join a controller (source role), Updates and Quit.
  - On Linux, tray click events aren't available; only the menu works. GNOME needs the AppIndicator extension.
- **Taskbar button** while the window is open. Closing the window hides to the tray (a setting, on by default). `--minimized` starts in the tray. Only one instance runs at a time.
- **Start at sign-in** is opt-in. It is per-user (HKCU Run on Windows, XDG autostart on Linux), set explicitly rather than through the plugin's machine-wide default. It can be changed from the installer and from the UI through the app's local bridge.
- **Node management:**
  - If no node answers on loopback and none is installed as a service, the app starts its bundled node as a managed launch (`--managed`, [settings layering](control-plane.md#settings-layering)) and restarts it if it crashes.
  - The app stops the node when the app exits.
- **Hosts:** a built-in screen for adding, listing and switching controllers. Client keys are stored in the operating system's credential store.
- **Portable mode:** a `portable.ini` file next to the executable keeps all data in `.\data`, and the service option is hidden.

## Services

Services start the node with `--service`, which picks the service folders ([folders](control-plane.md#folders)).

- **Windows:** the node runs as a Windows service through the `windows-service` crate:
  - name `FOLO`, delayed automatic start
  - restarts after failures (5 s, three times, counter reset daily)
  - data in `%ProgramData%\FOLO`
  - The service account is decided by a verification step: a virtual service account if loopback audio capture works under it, otherwise LocalSystem.
- **Linux:**
  - `folo.service` is a **systemd user unit**: it runs the node with a local source and can capture audio from the user's PipeWire.
  - `folo-controller.service` is a system unit for headless controllers, running as user `folo` with data in `/var/lib/folo`.
  - The capture agent is a user unit named `app-<app-id>-capture.service`, `PartOf=` and `After=graphical-session.target`.
- **Default for desktops is user mode** (start at sign-in, if enabled). Service mode suits headless controllers and audio-only sources, because **screen capture needs a signed-in session** ([display capture](../integrations/display-capture.md#where-capture-runs)).

## Artifacts per release

| Platform | Installable | Portable |
|---|---|---|
| Windows x64 | NSIS installer (`FOLO_<v>_x64-setup.exe`) | `FOLO_<v>_x64-portable.zip`; headless `folo_<v>_windows_x64.zip` |
| Linux x64 and arm64 | `.deb`, `.rpm`; Arch package `folo-bin` (AUR, repackaging the release tarball) | `.AppImage`; headless `folo_<v>_linux_<arch>.tar.gz` |
| Docker | `ghcr.io/d-pranoy/folo:<v>` (amd64, arm64) | n/a |
| All | `SHA256SUMS`, `latest.json`, `folo-update.json`, `.sig` files, provenance attestations | |

## Windows installer (NSIS)

- **Install mode:** `perMachine` (Program Files), because installing a service needs administrator rights. WebView2 uses `embedBootstrapper`.
- **Prompts** (in the `NSIS_HOOK_POSTINSTALL` hook, with `/SD IDNO` defaults for silent installs):
  1. "Run FOLO as a background service? It starts with Windows, before sign-in. Screen sync works only after you sign in."
  2. "Start the FOLO app when you sign in?"
- **Flags for silent installs:** `/SERVICE=0|1`, `/AUTOSTART=0|1`. Updates (`/UPDATE`) skip the prompts and keep the earlier choices, which are stored in `HKLM\Software\FOLO`.
- **Hooks:**
  - `PREINSTALL` stops the service and any running node or worker processes, so files can be replaced.
  - `PREUNINSTALL` stops the service; it removes the service and the start-at-sign-in entry unless this is an update.
- **Firewall:** a Private-profile rule is added only when remote access is first turned on, after the user confirms in the UI.

## Linux packages

- **deb and rpm:**
  - Units are installed **disabled**, to `/usr/lib/systemd/{user,system}/`.
  - The node and its workers go to `/usr/lib/folo/`.
  - `postinst` runs `systemctl daemon-reload` only (Debian policy: no interactive prompts).
  - The app's first run asks about the background service and start at sign-in.
  - Runtime dependencies: `libpipewire-0.3`, `libpulse0` (audio capture, [ADR-0031](../decisions/ADR-0031-linux-audio-capture.md); `pulseaudio-libs` in the rpm), `xdg-desktop-portal` (plus the KDE or GNOME backend), `libwebkit2gtk-4.1`, `libayatana-appindicator3`.
- **Arch / AUR:** `packaging/arch/PKGBUILD` for `folo-bin`. It repackages the signed release tarball, ships the same units and depends on `pipewire`, `libpulse`, `xdg-desktop-portal`, `webkit2gtk-4.1` and `libayatana-appindicator`. CI validates it with `makepkg` in an Arch container.
- **AppImage:** start at sign-in uses an XDG entry with the `$APPIMAGE` path, with a warning if the file moves. It links the system's libpipewire.
- Desktop builds are compiled on Ubuntu 22.04 runners for a low glibc baseline.

## Docker (controller)

- The controller-only node has no audio or capture dependencies. It is a static musl binary in a distroless image, run as a non-root user.
  - volume: `/data`
  - port: `8777`
  - default `FOLO_ROLE=controller`, local source off, and `FOLO_HOME=/data` ([folders](control-plane.md#folders))
  - health check on `/api/v1/health`
- `compose.yaml` recommends host networking (mDNS, Hue and WLED discovery); a bridge-network variant uses manual addresses.
- The first start logs the admin key and how to create invites.
