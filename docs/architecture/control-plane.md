# Control plane

Status: Accepted · Implementation: Partial (the node's settings, folders, documents, identity, local link, supervision, logs, command line and client API; no pairing or discovery yet)

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
- While remote access is off, a node binds `127.0.0.1` only ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D6).
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
| **Managed** (started by the desktop app with `--managed`) | defaults → `node.toml`. The node takes no other flags, ignores inherited `FOLO_*` variables and removes them from its environment. |
| **Foreground** (CLI, service, Docker) | defaults → `node.toml` → `FOLO_*` environment → CLI flags |

- Values from the environment or flags are **deployment overrides**. The UI shows them locked, with their source, and `folo status --json` lists them.
- **Variables and flags** ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D3):

  | Setting | Variable | Flag |
  |---|---|---|
  | `role` | `FOLO_ROLE` | `--role controller\|source` |
  | `listen.address` | `FOLO_LISTEN_ADDRESS` | `--address <address>` |
  | `listen.port` | `FOLO_LISTEN_PORT` | `--port <port>` |
  | `data_dir` | `FOLO_DATA_DIR` | `--data-dir <folder>` |
  | `remote_access.enabled` | `FOLO_REMOTE_ACCESS_ENABLED` | `--remote-access on\|off` |
  | `remote_access.allowed_hosts` | `FOLO_REMOTE_ACCESS_ALLOWED_HOSTS`, comma-separated | `--allowed-host <host>`, repeated |
  | `log_level` | `FOLO_LOG_LEVEL` | `--log-level <level>` |
  | `local_source.enabled` | `FOLO_LOCAL_SOURCE_ENABLED` | `--local-source on\|off` |

  Variables take `true`, `on`, `yes` or `1` and `false`, `off`, `no` or `0` for booleans, in any case. An override the node can't read is ignored with a warning, so the layer below it stands; one out of range is clamped.
- List settings (hostnames, allowed origins) add up across sources instead of replacing each other, without duplicates, up to the setting's limit.
- `folo reload` applies hot-reloadable changes and reports those that need a restart (ADR-0034 D7):
  - applied while running: `log_level`, `remote_access.allowed_hosts`, `local_source.enabled` and the documents
  - after a restart: `role`, `listen`, `data_dir` and `remote_access.enabled`
  - Variables and flags stay as the node was started with them.

## Folders

Each node has a home folder holding `node.toml`, and a data folder holding the documents, `secrets.json` and `library/`. The node takes the first launch below that applies ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D1, following [ADR-0010](../decisions/ADR-0010-settings-persistence-and-migration.md) D2):

| Launch | Home | Data, by default | Logs |
|---|---|---|---|
| `--home <folder>` or `FOLO_HOME` | that folder | the home | `logs` in the data folder |
| Portable: `portable.ini` beside the executable | `data` beside the executable | the home | `logs` in the data folder |
| Service: `--service` | `%ProgramData%\FOLO` on Windows, `/var/lib/folo` on Linux | the home | `logs` in the data folder |
| User, Windows | `%LOCALAPPDATA%\FOLO` | the home | `logs` in the data folder |
| User, Linux | `$XDG_CONFIG_HOME/folo` | `$XDG_DATA_HOME/folo` | `$XDG_STATE_HOME/folo/logs`, or `logs` in the data folder when `data_dir` is set |

- `data_dir` moves the data folder. The XDG variables take their XDG Base Directory defaults when unset. The Docker image sets `FOLO_HOME=/data`.
- **One node per home:** a running node holds an operating-system lock on `folo.lock` in its home, which holds its process id. The lock ends with the process, also when it crashes. Experiments beside a live node use a home and a port of their own.

## Command line

`folo` without a command runs the node, as `folo run` does. Every command takes `--home <folder>` and `--service`, so it finds the same home as the node it is about. `folo status` and `folo reload` reach a running node through its loopback API, at `127.0.0.1` and the port that `node.toml`, `FOLO_LISTEN_PORT` and `--port` give, in that order ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D5).

| Command | What it does | Exit status |
|---|---|---|
| `folo run [flags]` | Runs the node until it is stopped | 0 after a clean stop; 1 on a failure; 3 when another node holds the home |
| `folo status [--json]` | The [status](#status-and-health), from the running node or, when none runs, from the home's files | 0 while a node runs; 1 when it doesn't answer; 3 when none runs |
| `folo reload` | Applies `node.toml` and the documents to the running node, and lists what needs a restart | 0 when reloaded; 1 on a failure; 3 when no node runs |
| `folo config import [<studio.json>]` | Imports a legacy `studio.json`, from the given file or else as a first run would. It takes the home's lock, so the node must be stopped. | 0 when imported; 1 on a failure; 3 while the node runs |
| `folo token show\|rotate` | The master admin key ([RB-15](../runbooks/m7-network.md#rb-15-client-access)) | |
| `folo pair reset`, `folo join <code>` | Pairing recovery and joining a controller ([RB-16](../runbooks/m7-network.md#rb-16-source-pairing-and-link)) | |

A usage error exits with status 2. Until their run books land, `token`, `pair` and `join` say so and exit with status 1.

**First run:** a node whose data folder has no `zones.json` imports the first `studio.json` it finds, in its home and then in its working directory, with `folo-source migrate-v1` (ADR-0034 D10, [ADR-0010](../decisions/ADR-0010-settings-persistence-and-migration.md) D5).

## Supervision

- The desktop app supervises a managed node, or a service manager (Windows service, systemd) supervises the node.
- The node supervises `folo-source`: it checks a heartbeat, restarts it with backoff, and stops it when disabled. Source logs appear in the controller's log viewer.
  - **Link:** the node starts it with the URL of its loopback source link and a launch token in its environment, and the engine connects back with that token. Its `status` messages, at least once a second, are the heartbeat ([source link](../specs/source-link.md#the-local-link), [ADR-0032](../decisions/ADR-0032-local-source-link.md)).
  - **Logs:** `folo-source` writes them to stderr as JSON lines, one object per record: `ts` (UTC, RFC 3339 with milliseconds), `level` (`debug`, `info`, `warning`, `error` or `critical`), `logger`, `msg`, and `exc` with the traceback when there is one.
  - **Stopping:** it shuts down cleanly on SIGINT, SIGTERM and Windows console control events (Ctrl+C, Ctrl+Break, closing its console), and joins its threads with timeouts.
- **Starting the engine** ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D9):
  - The node runs `folo-source` from its own executable's folder, or else from the `PATH`, with its own environment minus the `FOLO_*` variables, plus the link's URL and launch token.
  - It runs in the controller role while `local_source.enabled` is on. In the source role it always runs, and it gets no configuration until the node's owner controller sends one (RB-16).
  - **Restarts:** when it exits without being asked to, or is hung (5 s without a message or without reading one, or 30 s without linking after it starts or its link ends), the node starts a new one after 1 s, doubling up to 60 s, each delay cut by up to 25% at random. The delay starts over once the engine has stayed linked for 60 s. A hung engine is killed first.
  - **Stopping:** the node sends `restart {component: engine}`, which ends the engine with status 0, and kills it if it hasn't exited after 5 s.
- **The engine's configuration** is the `config.apply` the node builds from its documents. It sends it after the handshake, and again when a document it comes from changes or a zone is activated or deactivated:
  - `inputs`: the inputs of the source `local` in `sources.json`, or the defaults when there is none
  - `surfaces`: the surfaces in that source's `surfaces`, in that order, indexed from 0, **of effectively active zones only**: a zone that is inactive ([activation](spatial-model.md#zone-activation-rules)) has no surface assigned and nothing rendered, so a client can't preview it ([ADR-0036](../decisions/ADR-0036-controller-output.md) D6). Activating or deactivating a zone renumbers the later surfaces: the node drops the engine's frames it holds by index when the surfaces move, takes no surface frame from then until the engine answers the newest `config.apply` (an error answer counts), and the engine sends none under the old numbering after answering it. Each carries its zone's `beat_offset_ms` and `album_colors`, its role, and its zone's active scene in `show.json`.
  - `scenes`: the scenes those surfaces render
  - `publish`: each surface at its resolution (canvases at most 128 × 72), at 60 Hz, as `rgb8` and without LZ4 on loopback; the `spectrum` and `beat` signals and the surfaces' `levels`, while a live client subscribes to them ([client API](../specs/client-api.md#topics-and-delivery))
  - `revision`: the count of configurations the node has sent since it started
- **The node's log** ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D8):
  - It writes JSON lines in the engine's format to stderr and to `folo.log` in its log folder (see [folders](#folders)). The file rotates at 10 MiB, and four older files are kept (`folo.log.1` to `folo.log.4`).
  - The engine's stderr lines go into the node's log as they are. A line that isn't a JSON object is logged at `error` with the logger `folo-source`.
  - `log_level` filters the engine's lines as well as the node's own: `warning` counts as `warn` and `critical` as `error`.
- **Stopping the node:** SIGINT, SIGTERM, Windows console control events and the service manager's stop all stop it cleanly. It stops its workers, releases its outputs ([providers](providers.md#lifecycle)), writes its pending documents and releases its home's lock.
- `folo-capture` always runs in the signed-in user's session. The node starts it when the node itself runs in that session; otherwise the desktop app starts it at sign-in ([display capture](../integrations/display-capture.md#where-capture-runs)).
- **Media sessions** (now playing) also belong to the user's session. In Windows service mode, the effect engine runs outside it, so now playing reports `unavailable` in v1 ([ADR-0028](../decisions/ADR-0028-studio-features-carried-into-the-rebuild.md)).

## Status and health

- `folo status --json` prints the node's status, `NodeStatus` in the [client API](../specs/client-api.md), which `GET /api/v1/system/status` serves on loopback:
  - whether a node runs
  - id, version, role
  - home, data and log paths, listen address
  - managed or foreground launch, and the launch that chose the folders
  - the deployment overrides with their sources
  - pairing state, supervised workers, providers
- `GET /api/v1/health` needs no authentication and reveals no configuration.
