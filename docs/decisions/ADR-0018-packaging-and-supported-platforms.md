# ADR-0018: Packaging and supported platforms

Status: Accepted
Date: 2026-10-13

## Context

GitHub Actions must build executables and installers in portable and installable formats, and the Windows installer must ask about the service. Arch-based distributions (such as CachyOS with KDE Plasma on Wayland) are an important target. Tauri builds NSIS, MSI, deb, rpm and AppImage, but has no official portable mode and no pacman packages.

## Decision

- **D1.** **Supported platforms** ([platform and packaging](../architecture/platform-and-packaging.md#supported-platforms)):
  - **Tier 1:** Windows 11 x64; Ubuntu 24.04 (KDE, GNOME, Wayland); Arch Linux and derivatives (KDE Plasma, Wayland).
  - **Tier 2:** Windows 10 22H2, Ubuntu 22.04, Debian 12+, current Fedora, X11 sessions.
  - **Docker:** amd64 and arm64.
- **D2.** **Windows installer (NSIS):**
  - per-machine; WebView2 embedded bootstrapper
  - hook prompts for the **service** and **start at sign-in**, with `/SD` defaults
  - `/SERVICE=` and `/AUTOSTART=` flags for silent installs
  - no prompts on `/UPDATE`; earlier choices kept
- **D3.** **Windows portable:** a zip of the app and node with `portable.ini` (data next to the executable; no service). Headless node archives for Windows and Linux.
- **D4.** **Linux:**
  - deb and rpm with units installed disabled, and a first-run choice
  - AppImage
  - **an AUR package `folo-bin`** that repackages the signed release tarball, validated in CI in an Arch container
  - builds on Ubuntu 22.04 runners for a low glibc baseline
- **D5.** **Docker:** a static musl controller binary in a distroless image, non-root, `/data`, host networking recommended.
- **D6.** No MSI in v1.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| A custom NSIS template page | Nicer prompts | Forks Tauri's template; churn on every Tauri update |
| An MSI property instead of prompts | Good for managed deployment | No prompt |
| Flatpak | Sandboxed, distro-neutral | Portal app id and service units are harder; later option |

## Consequences

Install, upgrade and uninstall matrices run in CI ([RB-20](../runbooks/m8-release.md#rb-20-windows-service-and-installer), [RB-21](../runbooks/m8-release.md#rb-21-linux-packaging)).
