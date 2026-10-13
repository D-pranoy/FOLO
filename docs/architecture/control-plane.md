# Control plane

Status: Accepted · Implementation: Not started

Covers roles, ports, pairing, links, settings, supervision and status. Decided in [ADR-0003](../decisions/ADR-0003-system-roles.md), [ADR-0008](../decisions/ADR-0008-networking-pairing-and-encryption.md), [ADR-0009](../decisions/ADR-0009-client-access.md) and [ADR-0010](../decisions/ADR-0010-settings-persistence-and-migration.md).

## Roles

| Setting | Values | Default |
|---|---|---|
| `role` | `controller` or `source` | `controller` |
| `local_source.enabled` | Whether a controller runs a local source alongside itself | On for desktop installs, off in Docker |

- **Controller:** the full system; listens on the LAN.
- **Source:** links out to its owner controller (and any consumer controllers). Its local API binds to **loopback only**, for the desktop app and tray.

### Switching roles

| Path | How |
|---|---|
| UI | On controller B: System → **Join another controller**. Scan or paste controller A's invite, or pick A from the discovered list and approve on A. |
| Environment | `FOLO_ROLE=source` with `FOLO_JOIN=<invite>` (Docker, headless installs) |
| CLI | `folo --role source`, then `folo join <invite>` |

When a controller becomes a source:
1. It releases its outputs. Its controller configuration (zones, providers, scenes, library) is kept but dormant.
2. It **asks** before handing its own sources over to the new controller. The new controller issues a short-lived, multi-use handover invite, which B passes to its sources.
3. It offers to upload its scenes to the new controller as a pack.

Going back: the owner controller releases the source, or someone at the machine chooses **Leave controller**. The node returns to the controller role with its dormant configuration.

## Ports

- **Only the controller listens on the network**: one TCP port, default `8777`, carrying the HTTP API, the UI, WebSockets (live data, source links, ingest) and the unauthenticated health endpoint.
- Sources make outbound connections only. Optional mDNS browsing uses multicast and needs no inbound TCP port.
- Third-party ingest goes to the controller, or to a source over loopback only.

## Discovery

- **Controllers advertise** `_folo._tcp` with TXT keys:
  - `id`: the controller id
  - `pk`: its static public key
  - `api`: the API version
  - `name`
- **Sources may browse** for controllers to offer join requests. It can be turned off; invites carry addresses.

## Pairing a source

Formats are in [pairing and connect](../specs/pairing-and-connect.md).

| Method | Flow |
|---|---|
| **Invite** (primary) | The controller shows a QR code and link holding its addresses, its static public key, a one-time 128-bit secret and an expiry (10 minutes). The source uses it via scan or paste in its UI, `folo join`, or `FOLO_JOIN`. |
| **Join request** | The source finds a controller over mDNS and sends a join request. Both screens show a 6-digit verification code derived from the handshake. An admin approves on the controller. |

A source has exactly one **owner** controller, which holds admin rights, and optionally **consumer** controllers that only receive surfaces. Only the owner can:
- change the source's configuration
- install packs on it
- pair it with other controllers
- approve its updates

Recovery: `folo pair reset` or the local UI. You must be at the machine; it can't be done remotely.

## Source link

- A WebSocket from the source to `ws://<controller>:8777/api/v1/source-link`, encrypted end to end with **Noise IK**. The source knows the controller's static key from the invite or from mDNS. Traffic is protected even though the HTTP transport is plain.
- Carries:
  - controller-to-source commands (JSON-RPC 2.0)
  - source-to-controller status, activity, metrics and update progress
  - binary surface frames
- Specified in [source link](../specs/source-link.md).
- **Central management:** the controller stores every source's configuration (inputs, assigned surfaces, scenes, packs) and pushes it by revision. A source keeps the last configuration it applied.

## Settings layering

| Launch | Order (later wins) |
|---|---|
| **Managed** (started by the desktop app) | defaults → config file. Inherited `FOLO_*` variables are removed. |
| **Foreground** (CLI, service, Docker) | defaults → config file → `FOLO_*` environment → CLI flags |

- Values from the environment or flags are **deployment overrides**. The UI shows them locked, with their source.
- List settings (hostnames, allowed origins) add up across sources instead of replacing each other.
- `folo reload` applies hot-reloadable changes and reports those that need a restart.

## Supervision

- The desktop app supervises a managed node, or a service manager (Windows service, systemd) supervises the node.
- The node supervises `folo-source`: it checks a heartbeat, restarts it with backoff, and stops it when disabled. Source logs appear in the controller's log viewer.
- `folo-capture` always runs in the signed-in user's session. The node starts it when the node itself runs in that session; otherwise the desktop app starts it at sign-in ([display capture](../integrations/display-capture.md#where-capture-runs)).

## Status and health

- `folo status --json` reports:
  - id, version, role
  - home and data paths, listen address
  - managed or foreground launch
  - pairing state, supervised workers, providers, log path
- `GET /api/v1/health` needs no authentication and reveals no configuration.
