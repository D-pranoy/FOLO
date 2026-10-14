# Architecture overview

Status: Accepted · Implementation: Not started

## Components

| Component | Language | Runs | Responsibility |
|---|---|---|---|
| **Node** (`folo`) | Rust | Desktop (managed by the desktop app), Windows service, systemd unit, Docker | One binary with two roles. **Controller role:** zones, targets, providers, scenes, library, sources, projection, API, UI, updates. **Source role:** outbound link to a controller; supervises source workers. Both roles: identity, settings, status, supervision. |
| **Effect engine** (`folo-source`) | Python | Child of a node | Audio inputs (including app capture), gain and auto level, analysis, beat tracking, now playing, effects, surface rendering, publishing surfaces and signals to the node over loopback |
| **Capture agent** (`folo-capture`) | Rust | The signed-in user's session | On-demand display capture, GPU downscaling, HDR tone mapping, black-bar detection; small frames to the effect engine |
| **UI** | TypeScript (Svelte) | Served by the controller | Every management screen, version-locked to its controller |
| **Desktop app** | Rust (Tauri v2) | The signed-in user's session | Hosts the UI, keeps the list of controllers, tray icon, start at sign-in, manages the local node when it isn't a service |
| **Clients** | | Anywhere on the LAN | Desktop app, browsers, the future Android app, the Home Assistant integration |

See [ADR-0003](../decisions/ADR-0003-system-roles.md) and [ADR-0004](../decisions/ADR-0004-implementation-languages-and-contracts.md).

## Topology

```
 PC (source)                                     Controller host (desktop, server or Docker)            LAN devices
 ┌──────────────────────────────────┐             ┌─────────────────────────────────────────┐
 │ folo node (source role)          │  outbound   │ folo node (controller role)             │──► Hyperion (JSON/TCP)
 │  ├─ folo-source: audio → effects │  Noise/WS   │  zones · targets · surfaces · scenes    │──► WLED (DDP/UDP)
 │  │                 → surfaces    │ ══════════► │  source manager · compositor            │──► Hue Bridge (DTLS/UDP)
 │  └─ folo-capture: screen → frames│             │  projection · adapters · providers      │
 │ loopback API only (local app)    │             │  API + UI on the LAN (only open port)   │
 └──────────────────────────────────┘             └─────────────────────────────────────────┘
        ▲ desktop app (tray, UI host)                    ▲  clients: desktop app · browser · Android (later) · HA (later)
```

On a desktop all-in-one install, the controller runs a **local source** (its own effect engine and capture agent) alongside itself. Local and remote sources use the same protocol.

## Data flow per frame

1. **Inputs.** The effect engine reads audio packets (about every 10 ms). The capture agent delivers small screen frames on demand.
2. **Effects.** Layers render into surfaces at the input rate.
3. **Publishing.** Surfaces are published at the resolution and rate each zone requests (default 60 Hz).
4. **Compositing.** The controller picks or blends sources per surface (priority, activity, crossfade), then applies each zone's reactive mode and ambient light.
5. **Projection.** Each target element samples its surface through a precomputed gather plan.
6. **Adapters.** Colour model, resolution, rate, calibration, latency offset, flash limiter.
7. **Providers.** Each sends at its own rate (Hyperion 40 Hz, WLED 60 Hz, Hue 50 Hz).

Details: [pipeline](pipeline.md), [spatial model](spatial-model.md), [capabilities](capabilities.md).

## Control plane

- Only the controller listens on the network, on one port (default 8777) for the API, UI, source links and ingest.
- Sources connect out. They pair with an invite or an approved join request, and the link is encrypted with Noise.
- Clients use per-client API keys over LAN-only HTTP.

Details: [control plane](control-plane.md), [security](security.md).

## Process supervision

```
desktop app ──manages──► node (unless installed as a service; then the service manager supervises it)
node ──supervises──► folo-source (heartbeat, restart with backoff)
node or desktop app ──starts on demand──► folo-capture (always in the user session)
```

## Related documents

- [Platform and packaging](platform-and-packaging.md): how these components ship and start.
- [Updates](updates.md): how they update.
- [Projection-mapping groundwork](projection-mapping-groundwork.md): what is built now to avoid churn later.
- [Current system](current-system.md): the code that exists today.
