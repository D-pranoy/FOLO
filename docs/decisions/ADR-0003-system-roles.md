# ADR-0003: System roles: controller, source, clients

Status: Accepted
Date: 2026-10-13

## Context

Audio and screens are captured on the computers where they play. Lights are driven from one place, which can be a desktop, a server or a Docker container. Users manage everything from desktops, browsers and phones. Display capture works only in the signed-in user's session. Network exposure should be minimal.

## Decision

- **D1.** One node binary (`folo`) with two roles, **controller** and **source**, chosen by the `role` setting.
- **D2.** **Sources run inputs and effects and publish surfaces** (lighting data) to their controller. **Audio is never relayed** between machines, and full video frames never leave the capturing machine.
- **D3.** **The controller** projects surfaces onto targets, drives the providers, owns zones, scenes and the library, centrally manages its sources (configuration, packs, pairing, updates) and serves the API and UI.
- **D4.** **Only the controller listens on the network.** Sources connect out; their local API binds to loopback only.
- **D5.** By default, a controller **runs a local source alongside itself** and supervises its runtime (`local_source.enabled`: on for desktop installs, off for Docker).
- **D6.** **Role switching:**
  - A controller becomes a source by joining another controller, from its UI, through `FOLO_ROLE` with `FOLO_JOIN`, or through `--role` with `folo join`.
  - Its controller configuration stays dormant.
  - It asks before handing over its own sources.
  - Leaving restores the controller role.
- **D7.** Source work happens in supervised worker processes:
  - `folo-source`, the effect engine
  - `folo-capture`, the display capture agent, which always runs in the user session
- **D8.** Clients (desktop app, browsers, the Android app later, Home Assistant later) talk only to controllers.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Separate binaries per role | Smaller binaries | Role switching needs reinstalling; more packaging |
| Sources relay audio to the controller | Analysis in one place | Rejected by product decision; bandwidth and privacy |
| Sources listen and the controller dials them | Simple discovery | Opens ports on every PC |
| Everything in one process | Fewer moving parts | Audio analysis jitter in output timing; capture can't run in a service |

## Consequences

- One protocol serves both local and remote sources.
- Desktop installs default to user mode, because screen capture needs a session ([ADR-0017](ADR-0017-desktop-app-tray-autostart-and-service.md)).
- Details: [control plane](../architecture/control-plane.md).
