# Source link

Status: Draft · Implementation: Partial (the AsyncAPI contract, message fixtures, the conformance harness, and both sides of the local link) · Contract version: 1.3

The connection between a source node and a controller. Decided in [ADR-0008](../decisions/ADR-0008-networking-pairing-and-encryption.md).

## Transport

- The source dials `ws://<controller>:<port>/api/v1/source-link`. Only the controller listens.
- WebSocket subprotocol: `folo.source-link.v1`.
- All messages after the handshake are carried inside **Noise** transport messages. Each WebSocket binary message holds one Noise message (at most 65,535 bytes).
- Reconnect with exponential backoff (1 s up to 60 s, with jitter). The controller address is learned from the invite and refreshed from mDNS.

### The local link

A node's own effect engine uses the same endpoint on loopback, `ws://127.0.0.1:<port>/api/v1/source-link`, without Noise ([ADR-0032](../decisions/ADR-0032-local-source-link.md)):
- **Launch:** the node starts the engine with the link's URL and a random 256-bit launch token (base64url) in its environment, `FOLO_SOURCE_LINK` and `FOLO_SOURCE_TOKEN`, never on its command line.
- **Handshake:** the first message each way is the handshake payload as JSON, in a WebSocket text message. The engine's purpose is `local`, with the token. The node compares it in constant time, accepts one engine connection at a time and closes any other.
- **Messages:** each WebSocket binary message after that is one message, as below, without the Noise layer. `config.apply` follows the node's response, and the engine renders nothing before it.
- **Heartbeat:** the engine sends `status` at least once a second; after 5 s without any message the node treats it as hung.
- **Losing the node:** the engine reconnects with the backoff above, and exits after 30 s without its node.
- **Refused:** a response other than `ok` ends the engine with a non-zero exit status. The node starts a new one with a new token.
- **Exit status** of `folo-source`: 0 after a stop request or a restart the node asked for; 1 when the link was refused or the node lost; 2 when the environment lacks the link's URL or token.
- **What the engine answers:**
  - `config.apply`: clamped like any document the node sends, then `{}`.
  - `inputs.list`: the input listing.
  - `media.control`: `{}`, or the error `-32000` when no player took the action.
  - `media.art`: the art shown now, or the error `-32000` when `art_id` isn't it any more.
  - `restart {component: engine}`: `{}`, then it exits for the node to start it again. Other components get `-32602`.
  - Every other method gets `-32601`; they belong to the node.

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
    - `local` with the launch `token`, on [the local link](#the-local-link) only
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

A source answers a method it doesn't implement with the JSON-RPC error `-32601` (method not found), and params it can't take with `-32602` (invalid params). It logs and drops messages it can't read, which have no id to answer.

### Controller → source (JSON-RPC methods)

| Method | Purpose |
|---|---|
| `config.apply {revision, inputs, surfaces, scenes, publish}` | The full source configuration. Each assigned surface carries its zone's `beat_offset_ms` and `album_colors`, which its layers use. `publish` holds the requested resolution, rate and compression per surface, the signals to send with their signal index, and the surfaces whose layers' levels to send (`levels`). |
| `packs.install {id, version, url \| inline, sha256}`, `packs.remove {id}` | [Content distribution](#content-distribution); not sent in v1 |
| `inputs.list` | The inputs available on the source (outputs, inputs, apps playing audio, screens, media session), with capabilities |
| `media.control {action}` | `play_pause`, `next` or `previous` on the shown media session |
| `media.art {art_id}` | Returns the album art image (base64) for clients |
| `pair.with {invite}`, `pair.remove {controller_id}` | Owner only |
| `update.check`, `update.approve {version}`, `update.cancel` | Owner only |
| `logs.tail {lines, follow}` | Diagnostics |
| `restart {component}` | Restart a worker or the node |

### Content distribution

Decided in [ADR-0038](../decisions/ADR-0038-content-library.md) D22. In v1 the controller sends no packs: an applied scene is a complete copy and reaches the source in `config.apply`. A source that doesn't implement the methods answers `-32601`. When content a source must hold arrives ([content](../architecture/content.md#later)), they work like this:
- **`packs.install`:** `inline` carries the pack's canonical `.folopack` in base64, and `sha256` its pack hash ([pack format](pack-format.md#validation)). `url` is reserved and not sent. The source checks the hash before anything else and answers `-32000` with the message `hash_mismatch` when it differs. It then validates the pack as the controller's library does, keeps it in its own `library/<author>/<name>/`, replacing any version it had, and answers `{}`.
- **`packs.remove`:** removes the pack and answers `{}`, also when the source doesn't have it.

### Source → controller (notifications)

| Notification | Purpose | Sent |
|---|---|---|
| `status {state, workers, inputs, errors}` | Health | Once a second, and when it changes (the local link's heartbeat) |
| `activity {surface?, active: bool, level_db?, quiet_s}` | Activity for compositor switching and the zone's ambient light (silence uses the level without gain) | 10 times a second, and when `active` changes |
| `media {state, app, title, artist, album, art, colors}` | Now playing | When it changes |
| `input {format, packets_per_s, delay_ms, glitches, peaks, clip, gain_db, auto_gain_db, state}` | Audio input status | At most 10 times a second |
| `levels {surface, layers: {id: [level_db, value, average_db]}}` | The levels of a surface's layers: the band level in dB, the shaped value 0–1 and the band's recent average in dB, which clients draw on their analyzers ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D6) | For each surface in `publish.levels`, with rendered steps, at most 60 times a second |
| `metrics {render_ms, publish_fps, dropped}` | Performance since the last one: the mean render time of a step, surface frames sent per second per published surface, and frames dropped (steps merged to keep up, and frames replaced before they were sent) | Once a second |
| `update.available / progress / result` | Updates | As they happen |

## Surface frames

All integers are little-endian:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `FOLS` |
| 4 | 1 | Frame format version (1) |
| 5 | 1 | Pixel format: `1` rgb8, `2` rgb16, `3` rgbf32 |
| 6 | 1 | Flags: bit 0 LZ4-compressed payload (in the LZ4 frame format, which records the content size; sent only when it's smaller), bit 1 key frame |
| 7 | 1 | Reserved (0) |
| 8 | 2 | Surface index (as assigned in `config.apply`) |
| 10 | 2 | Width (cells) |
| 12 | 2 | Height (1 for strips) |
| 14 | 2 | Reserved (0) |
| 16 | 4 | Sequence number |
| 20 | 8 | Timestamp in µs (source monotonic clock; the controller tracks the offset) |
| 28 | n | Payload: `width × height × channels × bytes per channel`, row-major |

**Pixel formats:** the payload holds each cell's red, green and blue, in that order. The effect engine's values (0–255, [pipeline](../architecture/pipeline.md#frames-and-signals)) become:
- `rgb8`: each value truncated toward zero, as today's engine does, so parity mode gives today's bytes
- `rgb16`: `⌊value × 257⌋` (255 → 65,535), unsigned little-endian
- `rgbf32`: the values as they are, float32 little-endian

**Sequence and key frames:**
- A surface's sequence number counts the frames sent for it, from 0 when the source starts, wrapping at 2³². It doesn't start over when the link reconnects.
- Format version 1 has no delta frames: every frame is complete and has the key frame bit set.

**Delivery semantics:**
- **Newest wins.** The controller keeps only the latest frame per surface and drops late or duplicate sequence numbers.
- A surface whose source sends nothing for its staleness timeout (default 500 ms) counts as inactive.
- **Renumbering:** the surfaces of a `config.apply` are numbered in its order, so a new call can give a surface another index. The source sends no frame under the old numbering after it has answered the call, and the controller drops the frames it received before the answer, which belong to the old numbering.

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

A signal's sequence number counts the frames sent for it, as for surfaces.

The same newest-wins delivery applies. Used for:
- **The beat signal:** `[bpm, confidence, locked, phase, bar_beat]`, sent with every analysed packet while the beat tracker runs. Clients show it.
- **The spectrum signal:** 192 band levels in dB, sent with rendered steps, at most 60 times a second ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D6). Band `i` spans 20 Hz × 1000^(i/192) to 20 Hz × 1000^((i+1)/192). Its level is the loudest bin in that range, interpolated when the band is narrower than a bin, with the input's gain, and at least −120 dB. Clients draw it as the analyzer.
- **Activity levels.**
- Later, exporting analysis signals.

## Local ingest

Programs on a source machine may push signals or frames to the local node over loopback (`ws://127.0.0.1:<port>/api/v1/ingest/local`, with a local token), using the same frame formats. Local ingest never listens on the LAN.
