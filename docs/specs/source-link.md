# Source link

Status: Draft · Implementation: Partial (the AsyncAPI contract, message fixtures and the conformance harness; no implementation yet) · Contract version: 1.1

The connection between a source node and a controller. Decided in [ADR-0008](../decisions/ADR-0008-networking-pairing-and-encryption.md).

## Transport

- The source dials `ws://<controller>:<port>/api/v1/source-link`. Only the controller listens.
- WebSocket subprotocol: `folo.source-link.v1`.
- All messages after the handshake are carried inside **Noise** transport messages. Each WebSocket binary message holds one Noise message (at most 65,535 bytes).
- Reconnect with exponential backoff (1 s up to 60 s, with jitter). The controller address is learned from the invite and refreshed from mDNS.

## Handshake

- **Pattern:** `Noise_IK_25519_ChaChaPoly_BLAKE2s`.
  - The source is the initiator. It already knows the controller's static key from the invite or mDNS.
  - The controller is the responder.
- **Handshake payload from the source:** CBOR or JSON with:
  - `version`: the source-link versions it supports
  - `node`: id, name, software version, platform
  - `purpose`: one of
    - `connect` (already paired)
    - `invite` with `proof = HMAC-SHA256(invite_secret, handshake_hash)`
    - `join_request`
- **Controller response payload:**
  - `version`: the chosen version
  - `result`: `ok`, `pending` (join request) or `rejected`
  - for join requests: `verification_code`
- **Verification code:** both sides compute the first 6 decimal digits of `BLAKE2s(handshake_hash ‖ "folo-verify")` and show them for comparison.
- **After pairing,** a `connect` from a source whose static key isn't paired is rejected.

## Messages (inside the Noise channel)

Each message starts with one byte for its type:
- `0x01`: JSON-RPC 2.0 (UTF-8 JSON)
- `0x02`: surface frame (binary)
- `0x03`: signal frame (binary)

### Controller → source (JSON-RPC methods)

| Method | Purpose |
|---|---|
| `config.apply {revision, inputs, surfaces, scenes, publish}` | The full source configuration. Each assigned surface carries its zone's `beat_offset_ms` and `album_colors`, which its layers use. `publish` holds the requested resolution, rate and compression per surface, and the signals to send with their signal index. |
| `packs.install {id, version, url \| inline, sha256}`, `packs.remove {id}` | Content distribution |
| `inputs.list` | The inputs available on the source (outputs, inputs, apps playing audio, screens, media session), with capabilities |
| `media.control {action}` | `play_pause`, `next` or `previous` on the shown media session |
| `media.art {art_id}` | Returns the album art image (base64) for clients |
| `pair.with {invite}`, `pair.remove {controller_id}` | Owner only |
| `update.check`, `update.approve {version}`, `update.cancel` | Owner only |
| `logs.tail {lines, follow}` | Diagnostics |
| `restart {component}` | Restart a worker or the node |

### Source → controller (notifications)

| Notification | Purpose |
|---|---|
| `status {state, workers, inputs, errors}` | Health |
| `activity {surface?, active: bool, level_db?, quiet_s}` | Activity for compositor switching and the zone's ambient light (silence uses the level without gain) |
| `media {state, app, title, artist, album, art_id, colors}` | Now playing, sent when it changes |
| `input {format, packets_per_s, delay_ms, glitches, peaks, clip, gain_db, auto_gain_db, state}` | Audio input status, at most 10 Hz |
| `metrics {render_ms, publish_fps, dropped}` | Performance |
| `update.available / progress / result` | Updates |

## Surface frames

All integers are little-endian:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `FOLS` |
| 4 | 1 | Frame format version (1) |
| 5 | 1 | Pixel format: `1` rgb8, `2` rgb16, `3` rgbf32 |
| 6 | 1 | Flags: bit 0 LZ4-compressed payload, bit 1 key frame |
| 7 | 1 | Reserved (0) |
| 8 | 2 | Surface index (as assigned in `config.apply`) |
| 10 | 2 | Width (cells) |
| 12 | 2 | Height (1 for strips) |
| 14 | 2 | Reserved (0) |
| 16 | 4 | Sequence number |
| 20 | 8 | Timestamp in µs (source monotonic clock; the controller tracks the offset) |
| 28 | n | Payload: `width × height × channels × bytes per channel`, row-major |

**Delivery semantics:**
- **Newest wins.** The controller keeps only the latest frame per surface and drops late or duplicate sequence numbers.
- A surface whose source sends nothing for its staleness timeout (default 500 ms) counts as inactive.

## Signal frames

All integers are little-endian:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `FOLG` |
| 4 | 1 | Frame format version (1) |
| 5 | 1 | Reserved (0) |
| 6 | 2 | Signal index (as assigned in `config.apply`, or by `subscribed` on the live channel) |
| 8 | 2 | Count of values |
| 10 | 2 | Reserved (0) |
| 12 | 4 | Sequence number |
| 16 | 8 | Timestamp in µs (source monotonic clock) |
| 24 | 4 × count | Values, float32 |

The same newest-wins delivery applies. Used for:
- **The beat signal:** `[bpm, confidence, locked, phase, bar_beat]`, sent with every analysed packet while the beat tracker runs. Clients show it.
- **Activity levels.**
- Later, exporting analysis signals.

## Local ingest

Programs on a source machine may push signals or frames to the local node over loopback (`ws://127.0.0.1:<port>/api/v1/ingest/local`, with a local token), using the same frame formats. Local ingest never listens on the LAN.
