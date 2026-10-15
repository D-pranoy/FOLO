# Client API

Status: Draft · Implementation: Partial (the OpenAPI contract, generated TypeScript types and the conformance harness; no server yet)

The controller's API for clients: the UI, the desktop app, the Android app later, the Home Assistant client library, and scripts. It is served on the controller's single port (default 8777). The machine-readable form is `specs/openapi.yaml` ([specifications](README.md)).

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
| `/system/logs` | GET | Log viewer |
| `/system/backup`, `/restore` | GET, POST | Backup and restore |
| `/auth/login`, `/auth/logout` | POST | Browser sessions |
| `/auth/requests`, `/auth/requests/{id}/approve` | POST | Approval-based client pairing (used by Home Assistant) |

## Live WebSocket

`GET /api/v1/live`, upgraded to a WebSocket. Authenticated by cookie or by a first `auth` message.

- **Client → server:** `{"subscribe": [...]}` and `unsubscribe`. Topics:
  - `status`
  - `spectrum:<source>`, `levels:<source>`
  - `beat:<source>` (BPM, confidence, locked, phase, bar beat)
  - `media:<source>`
  - `input:<source>` (meters, stats, current gain and auto gain)
  - `zone:<id>` (reactive state: ambient fade 0–1, seconds of silence)
  - `surface:<id>`, `target:<id>`
- **Server → client:**
  - JSON messages: `status`, `levels`, `config_changed {document, revision}`, `event` (job progress, update states), and `subscribed`, which acknowledges a subscription and gives each binary topic its frame index.
  - Binary frames for pixel data and the spectrum, in the [source link's](source-link.md) frame formats: pixel data as surface frames (`FOLS`) and the spectrum as signal frames (`FOLG`, band levels in dB). The surface or signal index in a frame is the one `subscribed` gave its topic.
- **Envelope:** every JSON message from the server is `{"type": "<type>", "data": {...}}`. Client messages are `{"subscribe": [topics]}`, `{"unsubscribe": [topics]}` and `{"auth": {"key": "<client key>"}}`.
- **Rate:** about 30 Hz per client by default. Under backpressure the newest data wins.

## Ingest

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/ingest/surfaces` (WebSocket) | Third parties stream surface frames to the controller using a scoped ingest token. Framing as in the [source link](source-link.md#surface-frames). Appears as a source in the compositor. |
| `GET /api/v1/source-link` (WebSocket) | Source nodes ([source link](source-link.md)) |
