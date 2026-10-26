# M8 Release

Status: Accepted · Implementation: Not started

## RB-19: Desktop app

**Goal:** the Tauri v2 desktop app: UI host, tray, start at sign-in, hosts, node management.
**Depends on:** RB-11, RB-15.

1. **Scaffold `shell/`:** Tauri 2.x (pinned) with the identifier `io.github.d-pranoy.folo`, plus a minimal built-in frontend (Hosts screen, connection errors, first run). Capabilities are locked down; the IPC bridge only accepts the loopback origin.
2. **Node management:** if nothing answers on loopback and no service is installed, start the bundled `folo` from resources; hold the child, restart it after crashes with backoff, and stop it on exit. Stop any running node in installer pre-hooks.
3. **Tray:**
   - status icon (ok, warning, off, updating)
   - menu: Open, Output on/off, Start at sign-in, Join a controller (source role), Updates, Quit
   - a "screen sync active" indicator while the source's screen status says `capturing` or `paused` ([RB-14](m6-screen.md), [ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D21)
   - on Linux, menu only; document the GNOME AppIndicator extension
4. **Window:** a taskbar button while shown; close hides to the tray (setting); `--minimized`; single instance.
5. **Start at sign-in:** an explicit per-user implementation (HKCU Run / XDG autostart), off by default, exposed to the UI through the loopback bridge.
6. **Hosts:**
   - add hosts by connect code (scan through the camera where available, or paste)
   - several addresses per host; switching the active host
   - keys in the OS credential store (`keyring`)
7. **Capture agent:** when the node runs as a service, the app starts `folo-capture` at sign-in if screen inputs are enabled.
8. **Portable mode:** `portable.ini` beside the executable sets the data folder to `.\data` and hides service options.
9. **Tests:** Rust unit tests and a manual QA checklist for tray, window and start-at-sign-in behaviour on Windows 11, KDE and GNOME.

**Done when:** the QA checklist passes on Tier 1 platforms.

## RB-20: Windows service and installer

**Goal:** the node as a Windows service and an NSIS installer that asks about it.
**Depends on:** RB-06, RB-19.

1. **Verification step:** capture 5 s of loopback audio under LocalSystem and under a virtual service account, and check the level.
   - Also check whether **per-app capture** and **media sessions** work from the service. The expectation is that media sessions don't ([ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)).
   - Record the chosen account in [ADR-0017](../decisions/ADR-0017-desktop-app-tray-autostart-and-service.md) through a superseding ADR if it differs from the default.
2. **Service:**
   - `folo service install|uninstall|start|stop|status` through the `windows-service` crate
   - name `FOLO`, delayed automatic start, restart on failure (5 s, three times, reset daily)
   - data in `%ProgramData%\FOLO`; errors also go to the Event Log
3. **Tauri bundle:** `installMode: perMachine`, `installerHooks: nsis/hooks.nsh`, `webviewInstallMode: embedBootstrapper`. The node and its workers are bundled as resources.
4. **`hooks.nsh`:**
   - **PREINSTALL:** stop the service and any node or worker processes.
   - **POSTINSTALL** (not on `/UPDATE`):
     - read `/SERVICE=` and `/AUTOSTART=`, otherwise ask both questions with `MessageBox MB_YESNO … /SD IDNO` (texts in [platform and packaging](../architecture/platform-and-packaging.md#windows-installer-nsis))
     - apply the answers and store them in `HKLM\Software\FOLO`
     - on update, start the service again if it was installed
   - **PREUNINSTALL:** on update, stop the service only; otherwise stop and remove it and remove start at sign-in.
5. **Firewall:** a Private-profile rule, added only after confirmation when remote access is first turned on (elevated helper).
6. **Install-type marker** for the updater.
7. **CI matrix** on `windows-latest`:
   - silent install with `/S /SERVICE=1 /AUTOSTART=0` → `sc query FOLO` is running
   - upgrade keeps the choices
   - uninstall removes everything
   - interactive prompts covered by the QA checklist

**Done when:** the matrix is green and the QA checklist passes.

## RB-21: Linux packaging

**Goal:** deb, rpm, AppImage and an Arch package, with systemd units.
**Depends on:** RB-06, RB-14, RB-19.

1. **Units:**
   - `folo.service` (user unit, `Restart=on-failure`)
   - `folo-controller.service` (system unit, user `folo`, `/var/lib/folo`)
   - `app-io.github.d-pranoy.folo-capture.service` (user unit, `PartOf=`/`After=graphical-session.target`)
2. **deb and rpm** through Tauri's `files`:
   - units into `/usr/lib/systemd/{user,system}/`; the node and workers into `/usr/lib/folo/`
   - `postinst` runs `daemon-reload` only; units are not enabled
   - `prerm` stops the units
   - dependencies: `libpipewire-0.3`, `libpulse0` (`pulseaudio-libs` in the rpm), `xdg-desktop-portal` with a backend, `libwebkit2gtk-4.1`, `libayatana-appindicator3`
3. **First run** asks about the background service (`systemctl --user enable --now folo`) and start at sign-in. `folo service install --user` does the same.
4. **AppImage:** start at sign-in through XDG with the `$APPIMAGE` path (warn if it moves); links the system libpipewire.
5. **Arch:** `packaging/arch/PKGBUILD` for `folo-bin`, repackaging the release tarball with the same units and Arch dependency names (`pipewire`, `libpulse`, `xdg-desktop-portal`, `webkit2gtk-4.1`, `libayatana-appindicator`). CI validates it with `makepkg` in an `archlinux` container. Publishing to the AUR is a maintainer step in the release checklist.
6. **Build hosts:** `ubuntu-22.04` (x64) and `ubuntu-22.04-arm` (arm64).
7. **Tests:**
   - install the deb in Ubuntu 22.04 and 24.04 containers and check the layout (`dpkg -c`)
   - `systemd-analyze verify`
   - rpm smoke test in a Fedora container
   - `makepkg` and `namcap` in Arch
   - AppImage `--version`

**Done when:** every package installs and starts in its CI container, and live verification has passed on Ubuntu (KDE and GNOME) and an Arch-based KDE system.

## RB-22: Docker image

**Goal:** a small, multi-arch controller image.
**Depends on:** RB-07, RB-16.

1. **Build:** the node with a `controller-only` feature set (no audio or capture dependencies), as a static musl binary.
2. **`docker/Dockerfile`:**
   - multi-stage, ending in `gcr.io/distroless/static`
   - non-root user; `/data`; `FOLO_ROLE=controller`; local source off
   - `EXPOSE 8777`; health check on `/api/v1/health`
3. **`compose.yaml`:** host networking recommended, with a bridge-network variant using manual addresses.
4. **First start** logs the admin key and how to create invites.
5. **CI:** build amd64 and arm64 on native runners; run a container smoke test (health, key flow); scan the image.

**Done when:** both architectures pass the smoke test and the image manages a source in the integration test.

## RB-23: Release pipeline

**Goal:** one tag produces every artifact, signed, checksummed and attested.
**Depends on:** RB-19 to RB-22.

1. **`build.yml`** (reusable and manual), on a matrix of `windows-latest` (x64), `ubuntu-22.04` (x64) and `ubuntu-22.04-arm` (arm64):
   1. Build the UI.
   2. Freeze the effect engine (PyInstaller one-dir), including cffi, the `winrt` packages on Windows and `dbus-fast` and Pillow on Linux.
   3. Build the node and the capture agent.
   4. Bundle them into the Tauri app.
   5. `tauri build` (Windows: NSIS; Linux: deb, rpm, AppImage) with updater artifacts.
   6. Assemble the Windows portable zip and the headless archives.
   7. Write install-type markers and SHA-256 checksums; upload artifacts.
2. **Signing:** update signatures with the update signing key from secrets (`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). `folo-update.json` lists every artifact by install type and architecture.
3. **`release.yml`** (on `v*` tags):
   1. Call build.
   2. Create a draft release with notes from release-please.
   3. Upload artifacts, `SHA256SUMS`, `latest.json`, `folo-update.json` and signatures.
   4. Attest build provenance for every artifact.
   5. Publish after the `release` environment's approval.
4. **`docker.yml`:** multi-arch push to `ghcr.io/d-pranoy/folo` with tags `X.Y.Z`, `X.Y`, `latest` (stable) and `edge` (the default branch, `v1` during the rebuild); provenance (max mode), SBOM, attestations.
5. **`release-please.yml`:** the release PR and changelog; versions kept in sync across `package.json`, `Cargo.toml` and `tauri.conf.json`.
   - Pull requests and tags made with the default `GITHUB_TOKEN` don't trigger workflows: release PRs would get no CI, and its tags wouldn't start `release.yml`.
   - So the workflow uses a GitHub App installation token (for example through `actions/create-github-app-token`, pinned to a SHA). The repository owner creates a small FOLO release App and stores its app id and private key as secrets ([governance](../process/governance.md#key-custody)).
6. **Nightly:** `build.yml` on pushes to the default branch (`v1` during the rebuild, [ADR-0030](../decisions/ADR-0030-rebuild-on-a-v1-branch.md)), unsigned, artifacts kept 14 days, never published.
7. **Hardening:** minimal `permissions:` per job, actions pinned by full commit SHA and limited to GitHub-authored actions plus the [allowlist](../process/governance.md#allowed-actions) (every action this pipeline adds goes on that list in the same pull request), concurrency groups, Dependabot (uv, npm, cargo, Actions, Docker), dependency review, cargo-deny and cargo-audit. CodeQL comes from GitHub's default setup only; a CodeQL job in a workflow conflicts with it.
8. **Code signing hook:** a reserved `signCommand`, plus signing of the node, engine and agent executables before bundling, for when SignPath signing is available.
9. **Release checklist** (`docs/process/workflow.md`): the clean-room rewrite gate, AUR publishing, the client library publish, and documentation status updates.

**Done when:** tagging a beta produces every artifact with checksums and attestations, and the update dry-run (RB-17) consumes them.

## RB-24: Documentation completion

**Goal:** documentation matches the implementation, and users can install and operate FOLO from it alone.
**Depends on:** all previous run books.

1. Update every document's `Implementation:` status. Reconcile any drift found during implementation, through ADRs where decisions changed.
2. Rewrite `AGENTS.md` for the new architecture (commands, demo mode, fakes, safety rules) and `README.md`:
   - install per platform: Windows, Ubuntu/Debian, Fedora, Arch (AUR), AppImage, portable, Docker
   - first run, zones, pairing sources and clients, Hue and WLED setup, screen sync, updates
3. **User guides:** troubleshooting (for example the Hue stream being in use, Wayland re-prompts, protected content, a dark strip with everything else working).
4. **Operator run books:** releasing, rotating the update key and client keys, recovering a source, migrating from `studio.json`.
5. **`THIRD_PARTY_NOTICES`:** dependencies, the source of the bundled Hue root certificates, any copied code with its notices.
6. Optional: publish the docs as a site (mkdocs-material on GitHub Pages) from the default branch.

**Done when:** a fresh install on every Tier 1 platform can be completed from the documentation alone.
