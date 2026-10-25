# Client API

Status: Draft · Implementation: Partial (the OpenAPI contract, generated TypeScript types and the conformance harness; the node serves the API, the live channel and the UI's files, with `501` for the endpoints [before their run books](#before-their-run-books))

The controller's API for clients: the UI, the desktop app, the Android app later, the Home Assistant client library, and scripts. It is served on the controller's single port (default 8777). The machine-readable form is `specs/openapi.yaml` ([specifications](README.md)). How it is served before its later run books land is decided in [ADR-0035](../decisions/ADR-0035-serving-the-client-api.md).

## Conventions

- Base path `/api/v1`. JSON bodies; mutating requests must send `Content-Type: application/json`.
- **Authentication:**
  - `Authorization: Bearer <client key>` for programs.
  - A session cookie (HttpOnly, SameSite=Strict) after browser login.
  - Loopback requests need no key.
  - Unauthenticated: `/api/v1/health`, static UI files and the pairing endpoints, which use their own proofs.
- **Scopes:** `control` (everyday use) and `admin` (configuration, pairing, packs, updates).
- **Errors:** `{"error": {"code": "<machine code>", "message": "<text>", "details": {...}}}` with an appropriate HTTP status. A method a path doesn't support gets 405 with an `Allow` header ([RFC 9110, section 15.5.6](https://www.rfc-editor.org/rfc/rfc9110#name-405-method-not-allowed)).
- **Concurrency:** documents carry `revision` and `server_id`. Writers may send `If-Match: <revision>`; without it, the last writer wins, as today.
- **Body limit:** 1 MB by default.
- **Caching:** API responses carry `Cache-Control: no-store`, and every response carries `X-Content-Type-Options: nosniff`.

### Request checks

Every request goes through these checks in order. The first that fails answers:

| Check | Fails with |
|---|---|
| The `Host` header names the node: a loopback name, or one of `remote_access.allowed_hosts` while remote access is on ([security](../architecture/security.md#rules)) | 403 `host_not_allowed` |
| An `Origin` header, when the request has one, names the node the same way: requests from pages on other sites, `null` origins included, are refused ([security](../architecture/security.md#rules)) | 403 `origin_not_allowed` |
| The client's address: loopback always; the private ranges only while remote access is on | 403 `client_not_allowed` |
| Authentication: loopback clients and the unauthenticated endpoints need no key; every other request to `/api/` does. Until client keys and sessions arrive ([RB-15](../runbooks/m7-network.md#rb-15-client-access)), no key is valid. | 401 `unauthorized` |
| The resource exists | 404 `not_found` |
| The resource takes the method | 405 `method_not_allowed` |
| A mutating request sends JSON | 415 `json_required` |
| The body fits the limit | 413 `body_too_large` |

Routes that answer loopback clients only (`/system/status`, `/system/reload`) answer others with 403 `loopback_only`.

### Documents

The zones, show, providers and sources documents are read and written through their resources. Every write is clamped like any document ([configuration model](config-model.md#clamping-rules)):
- `PUT /show` replaces the whole document.
- `POST /zones` and `POST /providers` add an item. An id that is missing, empty or already taken gets a new one. The answer is 201 with the item as stored.
- A list that already holds as many items as its schema allows refuses another with 409 `list_full`.
- `PUT /zones/{id}`, `PUT /providers/{id}` and `PUT /sources/{id}` replace the item with that id, which keeps the id in the path. There is none: 404 `not_found`.
- `DELETE /zones/{id}` and `DELETE /providers/{id}` remove it (204), or answer 404 `not_found`.
- **`If-Match`** names the revision of the document the item is in, as `12` or `"12"`. When the document has moved on, the write is refused with 412 `revision_mismatch` and the current revision in `details.revision`.
- A body that isn't JSON gets 400 `bad_body`; one with a [structural error](config-model.md#clamping-rules) gets 422 `structural_error`.
- Each write gives the document its next revision and sends `config_changed` on the [live channel](#live-websocket). When zones, the show or the sources change, the node sends its engine the new configuration ([control plane](../architecture/control-plane.md#supervision)).

### Before their run books

An endpoint whose subsystem arrives with a later run book answers 501 `not_implemented`, with that run book in `details.runbook` ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D2):

| Endpoints | Run book |
|---|---|
| `/zones/{id}/activate`, `/deactivate`, `/identify`, `/test`; `/providers/discover`; `/providers/{id}/admin/...` | [RB-08](../runbooks/m3-controller-core.md#rb-08-zones-projection-and-adapters) |
| `/scenes/{id}/apply`; `/library/...`; `/system/backup`, `/system/restore` | [RB-09](../runbooks/m3-controller-core.md#rb-09-content-library) |
| `/providers/{id}/hue/pair` | [RB-13](../runbooks/m5-devices.md#rb-13-hue-provider-and-zone-sync) |
| `/clients/...`; `/auth/...`; `PUT /system` | [RB-15](../runbooks/m7-network.md#rb-15-client-access) |
| `DELETE /sources/{id}`; `/sources/invites`; `/sources/join-requests/...`; `/ingest/surfaces` | [RB-16](../runbooks/m7-network.md#rb-16-source-pairing-and-link) |
| `/updates/...` | [RB-17](../runbooks/m7-network.md#rb-17-ota-updates) |

Until then:
- `GET /system` reports `start_at_sign_in` as `false` (until [RB-19](../runbooks/m8-release.md#rb-19-desktop-app)), and the service's `installed` and `running` as whether this node was launched with `--service` (until [RB-20](../runbooks/m8-release.md#rb-20-windows-service-and-installer)).
- On the live channel, every zone reports `active` false, `ambient` 0 and `quiet_s` 0, every provider reports `off`, and `target:` topics carry nothing (until RB-08). Sources other than `local` report `offline` (until RB-16).

## Resources

| Path | Methods | Purpose |
|---|---|---|
| `/health` | GET | Liveness; no configuration revealed |
| `/info` | GET | Controller id, name, version, role, contract versions, capabilities |
| `/state` | GET | Everything a UI needs to start, in one call |
| `/zones`, `/zones/{id}` | GET, POST, PUT, DELETE | Zones with targets, surfaces and mappings |
| `/zones/{id}/activate`, `/deactivate` | POST | Zone activation, following the [activation rules](../architecture/spatial-model.md#zone-activation-rules) |
| `/zones/{id}/identify`, `/test` | POST | Identify a target; run test sweeps |
| `/show` | GET, PUT | Scenes per zone, master, background. A whole-document write keeps today's edit flow. |
| `/scenes/{id}/apply` | POST | Apply a scene to a zone, with role overrides |
| `/effects` | GET | The effect catalog with parameter schemas |
| `/providers`, `/providers/{id}` | GET, POST, PUT, DELETE | Provider instances |
| `/providers/discover` | POST | Run discovery (mDNS, Hue discovery) |
| `/providers/{id}/admin/...` | GET, POST | Basic device administration ([ADR-0014](../decisions/ADR-0014-device-administration-scope.md)) |
| `/providers/{id}/hue/pair` | POST | The link-button pairing flow, with progress |
| `/sources`, `/sources/{id}` | GET, PUT, DELETE | Sources, inputs, assignments, priorities |
| `/sources/{id}/inputs` | GET | Available inputs: outputs, inputs, apps currently playing audio, screens. The UI refreshes it every 5 s while the input editor is open. |
| `/sources/{id}/media` | GET | Now-playing state: off, unavailable, error, none, playing, paused or stopped; app, title, artist, album, art id, colours |
| `/sources/{id}/media/art` | GET | The current album art image. Clients add the art id to bust caches. |
| `/sources/{id}/media/control` | POST | `{"action": "play_pause" \| "next" \| "previous"}` on the source's shown media session |
| `/sources/invites` | POST | Create an invite ([pairing and connect](pairing-and-connect.md)) |
| `/sources/join-requests`, `/{id}/approve`, `/{id}/reject` | GET, POST | Join requests |
| `/library/packs`, `/library/packs/{id}` | GET, POST (install), DELETE | Packs; export through `GET …/{id}/export` |
| `/clients`, `/clients/{id}` | GET, POST (issue key and connect code), DELETE (revoke) | Client keys |
| `/updates` | GET | Update status for every node |
| `/updates/{node}/approve`, `/skip` | POST | Approvals |
| `/updates/policy` | GET, PUT | The update policy |
| `/system` | GET, PUT | Remote access, start at sign-in and service status (start at sign-in and service are loopback only) |
| `/system/status` | GET | The node's status, `NodeStatus`, as `folo status --json` prints it ([control plane](../architecture/control-plane.md#status-and-health)); loopback only |
| `/system/reload` | POST | Re-reads `node.toml` and the documents, as `folo reload` does, and lists what needs a restart; loopback only |
| `/system/logs` | GET | Log viewer |
| `/system/backup`, `/restore` | GET, POST | Backup and restore |
| `/auth/login`, `/auth/logout` | POST | Browser sessions |
| `/auth/requests`, `/auth/requests/{id}/approve` | POST | Approval-based client pairing (used by Home Assistant) |

- **`/info`:** `name` is the computer's host name. `contracts` holds the client API's version, the newest source-link version the node speaks and the configuration's `schema_version`. `capabilities` lists the optional features the node has.
- **`/effects`:** the built-in effects as the contract lists them (`x-folo-effects` in the show schema), each `params_schema` with its references resolved so it stands alone ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D3).
- **`/system/logs`:** the newest `lines` records of the node's log (`folo.log` and its rotated files), oldest first. `source` is `folo-source` for the effect engine's records and `node` for the node's own; the levels `warning` and `critical` read `warn` and `error`.

### Requests to sources

The node answers `/sources/{id}/inputs`, `/media/art` and `/media/control` through the source's link, with `inputs.list`, `media.art` and `media.control` ([source link](source-link.md#controller--source-json-rpc-methods)). It answers `/media` from the source's latest `media` notification.
- The source `local` is the node's own effect engine while the node runs it. When `sources.json` doesn't list it, `GET /sources/local` gives its defaults and `PUT /sources/local` adds it.
- Any other id that `sources.json` doesn't list gets 404 `not_found`.
- A source that isn't linked gets 503 `source_offline`, as does `/media` before the source's first `media` notification. A source that doesn't answer within 5 s gets 504 `source_timeout`.
- When the source has no such art any more, `/media/art` gets 404 `not_available`; when no player took the action, `/media/control` gets 409 `not_available`.
- `/media/art` answers with the image's bytes and the content type the source gives, when that is PNG, JPEG, WebP, GIF or BMP, with a Content Security Policy that sandboxes it. Any other type gets 502 `source_error`.

## Live WebSocket

`GET /api/v1/live`, upgraded to a WebSocket. Authenticated by cookie or by a first `auth` message.

- **Client → server:** `{"subscribe": [...]}` and `unsubscribe`. Topics:

  | Topic | Sent as |
  |---|---|
  | `status` | `status` messages: every source's state, every zone and every provider |
  | `zone:<id>` | `status` messages: that zone (reactive state: ambient fade 0–1, seconds of silence) |
  | `input:<source>` | `status` messages: the source's input (meters, stats, current gain and auto gain) |
  | `beat:<source>` | `status` messages: the source's beat (BPM, confidence, locked, phase, bar beat) |
  | `media:<source>` | `status` messages: the source's now playing |
  | `levels:<source>` | `levels` messages, one per surface of the source |
  | `spectrum:<source>` | signal frames: the source's [spectrum](source-link.md#signal-frames) |
  | `surface:<id>` | surface frames: the surface's pixels |
  | `target:<id>` | surface frames: the target's element colours |

- **Server → client:**
  - JSON messages: `status`, `levels`, `config_changed {document, revision}`, `event` (job progress, update states), and `subscribed`, which acknowledges a subscription and gives each binary topic its frame index.
  - Binary frames for pixel data and the spectrum, in the [source link's](source-link.md) frame formats: pixel data as surface frames (`FOLS`) and the spectrum as signal frames (`FOLG`, band levels in dB). The surface or signal index in a frame is the one `subscribed` gave its topic.
- **Envelope:** every JSON message from the server is `{"type": "<type>", "data": {...}}`. Client messages are `{"subscribe": [topics]}`, `{"unsubscribe": [topics]}` and `{"auth": {"key": "<client key>"}}`.
- **Rate:** about 30 Hz per client by default. Under backpressure the newest data wins.

### Topics and delivery

Decided in [ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D4 and D5:
- **A `status` message** holds what its client subscribed to, among the topics sent as `status` messages. A client subscribed to none of them gets no `status` messages. A source's `state` is `connected` while it is linked, `connecting` while it starts or links again, `offline` otherwise, and `pending` while its join request waits (RB-16).
- **`levels` messages** name their surface, so layers with the same id on two surfaces stay apart.
- **`config_changed` and `event`** go to every client, subscribed or not.
- **Names that don't exist yet:** a topic may name a source, zone, surface or target that doesn't exist. It is acknowledged, and its data flows once there is some. `surface:<id>` names the first surface with that id in `zones.json`'s order.
- **Updates:** each client gets at most 30 updates a second. Each update carries what changed since the last one: one `status` message, one `levels` message per surface and one frame per binary topic, each with the newest data. `config_changed` carries each document's newest revision, and `event` each job's newest state, so nothing queues up for a slow client.
- **Frames** go to clients as their source sent them, with the index their topic has on that connection and without LZ4.
- **`subscribed`** answers each `subscribe` with the topics it added, and the index of each binary topic: the lowest the connection isn't using. `unsubscribe` gets no answer.
- **Limits:** a connection holds at most 64 topics; those past the limit are left out of `subscribed`. A client message that isn't a `subscribe`, `unsubscribe` or `auth` message closes the connection with status 1008. A loopback client's `auth` message changes nothing.
- **Signals the node asks for:** the node asks a source for its spectrum, beat and levels only while a client subscribes to them ([control plane](../architecture/control-plane.md#supervision)).

## UI files

The node serves the UI's files outside `/api/`, with no key ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D7):
- `GET /` serves `index.html`. Every other path serves the file at that path in the UI's build, or gets 404.
- Each node build embeds the UI's build (`ui/dist`). A build without it answers `/` with a page that says so.
- **Caching:** files under `assets/`, named by their content hash, carry `Cache-Control: public, max-age=31536000, immutable`; every other file is `no-store`.
- **Content Security Policy:** every UI file carries the policy in [security](../architecture/security.md#rules).

## Ingest

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/ingest/surfaces` (WebSocket) | Third parties stream surface frames to the controller using a scoped ingest token. Framing as in the [source link](source-link.md#surface-frames). Appears as a source in the compositor. |
| `GET /api/v1/source-link` (WebSocket) | Source nodes ([source link](source-link.md)). The `local` purpose is taken from loopback clients only; until RB-16 brings the other purposes, clients on other machines get 403 `loopback_only` before the upgrade. |
