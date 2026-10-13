# ADR-0017: Desktop app, tray, start at sign-in and Windows service

Status: Accepted
Date: 2026-10-13

## Context

The desktop app must:
- bundle the controller and the source
- start at sign-in only when enabled
- show an icon on Windows while running
- offer installing the node as a service

Services can't show UI (Session 0 isolation) and can't capture the screen. Tauri v2 provides tray, single-instance and autostart support. The `windows-service` crate hosts Rust services.

## Decision

- **D1.** The desktop app is **Tauri v2**:
  - it hosts each controller's served UI and keeps the host list
  - it **manages the local node** when the node isn't a service: starts it, restarts it after crashes, stops it on exit
- **D2.** **Tray icon** whenever the app runs, showing status. **Taskbar button** while the window is open. Closing hides to the tray (a setting, on by default). Single instance.
- **D3.** **Start at sign-in is opt-in.** It is per-user (HKCU Run / XDG autostart) and can be changed in the installer and the UI.
- **D4.** **Windows service:**
  - the node, hosted with the `windows-service` crate
  - delayed automatic start, restart on failure
  - data in `%ProgramData%\FOLO`
  - The account is a virtual service account if loopback audio capture works under it, otherwise LocalSystem.
- **D5.** **Desktop installs default to user mode.** Service mode suits headless controllers and audio-only sources. With a service, screen sync starts when the user signs in and the app (or capture agent) starts.
- **D6.** Linux uses a systemd **user** unit for the node with a local source, a system unit for headless controllers, and a capture-agent user unit bound to `graphical-session.target`.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| A tray inside the node | Independent of the app | Impossible as a service; a second tray implementation |
| WinSW, NSSM or pywin32 service hosts | Familiar | Extra binaries or Python coupling; the node is Rust |
| Electron or pywebview shells | Mature, or one language | Size; DIY packaging |

## Consequences

The installer and the first-run wizard explain the service and screen-sync trade-off ([ADR-0018](ADR-0018-packaging-and-supported-platforms.md)).
