# Source link

Status: Draft · Implementation: Partial (the AsyncAPI contract, message fixtures, the conformance harness, both sides of the local link, and both sides of the capture link; the engine does not decode screen frames yet) · Contract version: 1.4

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
- **Messages:** each WebSocket binary message after that is one message, as below, without the Noise layer. `config.apply` follows the node's response, and the engine renders nothing before it. The node also forwards the capture agent's [screen frames](#screen-frames) to the engine here, as `0x04` messages.
- **Heartbeat:** the engine sends `status` at least once a second; after 5 s without any message the node treats it as hung.
- **Losing the node:** the engine reconnects with the backoff above, and exits after 30 s without its node.
- **Refused:** a response other than `ok` ends the engine with a non-zero exit status. The node starts a new one with a new token.
- **Exit status** of `folo-source`: 0 after a stop request or a restart the node asked for; 1 when the link was refused or the node lost; 2 when the environment lacks the link's URL or token.
- **What the engine answers:**
  - `config.apply`: clamped like any document the node sends, then `{}`.
  - `inputs.list`: the input listing. Its `screens` are empty: the node adds the capture agent's ([display capture](../integrations/display-capture.md#the-agent-and-its-link)).
  - `media.control`: `{}`, or the error `-32000` when no player took the action.
  - `media.art`: the art shown now, or the error `-32000` when `art_id` isn't it any more.
  - `restart {component: engine}`: `{}`, then it exits for the node to start it again. Other components get `-32602`.
  - Every other method gets `-32601`; they belong to the node.

### The capture link

The display capture agent `folo-capture` has a loopback endpoint of its own, `ws://127.0.0.1:<port>/api/v1/capture-link`, without Noise ([ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D1 to D3). Only a node's own agent uses it, and nothing on it ever goes to another machine. Like `/source-link` it needs no key, and the node applies the Host, Origin and client-address checks before the upgrade ([security](../architecture/security.md#rules)), so a web page can't open it; a client on another machine gets `403 loopback_only`, even with remote access on, and a request that isn't a WebSocket upgrade gets `400 websocket_required`.
- **Launch:** the node starts the agent with the link's URL and a random 256-bit launch token (base64url) in its environment, `FOLO_CAPTURE_LINK` and `FOLO_CAPTURE_TOKEN`, never on its command line.
- **Handshake:** the subprotocol is `folo.capture.v1`. The first message each way is JSON in a WebSocket text message. The agent sends `{version: ["1"], agent: {version, platform}, token}`; the node checks the token (in constant time) before it tests whether an agent is already connected, accepts one agent connection at a time, closes any other and answers `{version: "1", result: "ok" | "rejected"}`. A response other than `ok` ends the agent with status 1. (The node answers `rejected` only to a connection that isn't its agent, so it starts no new one for it.)
- **Limits:** the first message must arrive within 5 s, and no message may exceed 400,000 bytes. The engine's decoder checks every screen frame it gets (magic, version, pixel format, flags, size, length `36 + 3 × width × height`, bars) and drops one that fails, logging the reason without any pixels.
- **Messages:** each WebSocket binary message after that starts with a type byte, `0x01` for JSON-RPC 2.0 or `0x04` for a [screen frame](#screen-frames).
- **Heartbeat:** the agent sends `capture.status` at least once a second; after 5 s without any message the node treats it as hung and restarts it.
- **Losing the node:** the agent reconnects with the backoff above, and exits after 30 s without its node.
- **Exit status** of `folo-capture`: 0 after a stop request or a restart the node asked for; 1 when the link was refused or the node lost; 2 when the environment lacks the link's URL or token.

| Node → agent | Purpose |
|---|---|
| `screens.list` | The screens the agent can capture: `{screens: [Screen]}`, as `inputs.list` lists them |
| `capture.start {screen, width, height, rate_hz, black_bars, profile, restore_token?}` | Starts a capture. Sent again while capturing, `black_bars`, `profile`, `rate_hz` and the size change in place; another `screen` ends the session and starts a new one. The frames fit `width × height` (`width` at most 480, `height` at most 270) in the screen's aspect ratio, at most `rate_hz` (1–60) a second. `restore_token` is sent only when starting from `off`. Answers `{}` once accepted; the progress is in `capture.status`. |
| `capture.stop` | Stops the capture; the agent stays up and its state is `off` |
| `restart {component: capture}` | Answers `{}`, then the agent exits for the node to start it again. Other components get `-32602`. |

Methods the agent doesn't know get `-32601`, params it can't take `-32602`, and a request it can't carry out (an unknown screen) `-32000`.

| Agent → node | Purpose | Sent |
|---|---|---|
| `capture.status {state, backend, screen, fps, hdr, protected, bars, restore_token?, message?, settings_uri?}` | `state` is `off`, `starting`, `waiting_consent`, `capturing`, `paused` or `error`. `bars` are the black bars found, as fractions (0–1) of the frame. `restore_token` is Linux's new portal token, which the node keeps and never shows to clients. `settings_uri` points at the operating system's privacy setting when it refuses capture. | Once a second, and when it changes |

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
- `0x04`: screen frame (binary), on the loopback links only: the capture link and the node's local link to its engine, never inside Noise

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

## Screen frames

The capture agent's frames, on the [capture link](#the-capture-link) and forwarded unchanged to the engine on the local link. All integers are little-endian:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `FOLC` |
| 4 | 1 | Frame format version (1) |
| 5 | 1 | Pixel format: `1` rgb8 |
| 6 | 1 | Flags: bit 0 protected content, bit 1 HDR (the frame was tone-mapped), bit 2 held (the agent can't capture now and sends its last picture again) |
| 7 | 1 | Reserved (0) |
| 8 | 2 | Screen index (the screen's position in the agent's `screens.list` answer) |
| 10 | 2 | Width (cells) |
| 12 | 2 | Height (cells) |
| 14 | 2 | Reserved (0) |
| 16 | 4 | Sequence number |
| 20 | 8 | Timestamp in µs (the operating system's monotonic clock: `QueryPerformanceCounter` on Windows, `CLOCK_MONOTONIC` on Linux) |
| 28 | 2 | Black bar at the top, in rows |
| 30 | 2 | Black bar at the bottom, in rows |
| 32 | 2 | Black bar at the left, in columns |
| 34 | 2 | Black bar at the right, in columns |
| 36 | n | Payload: `width × height × 3` bytes, the red, green and blue of each cell, row-major |

- **Pixels:** sRGB-encoded SDR, averaged in linear light when the agent downscaled them. The picture isn't cropped: the bars are the rows and columns, counted from each side, that the agent found black, and are at most `height − 1` rows and `width − 1` columns together. All zero means none.
- **Size:** `width` is 1 to 480 and `height` 1 to 270. A capture's frames can change size (a mode change). The payload is never compressed.
- **Sequence:** it counts the frames the agent sent since it started, wrapping at 2³². The first frame after a new connection counts as newer than any before it. A frame the agent repeats (nothing changed) or holds is a new message, with the send time as its timestamp and the next sequence number. How old a frame is counts from its arrival at the engine. The engine takes frames in the order they arrive on its link, which is ordered, and doesn't compare sequence numbers, so an agent that starts again needs no special care.
- **Delivery:** newest wins on both hops. A frame replaced before it was sent is dropped.
- **Privacy:** a screen frame is never sent to a client, to another machine, to a log or to disk ([ADR-0037](../decisions/ADR-0037-display-capture-agent.md) D9).

## Local ingest

Programs on a source machine may push signals or frames to the local node over loopback (`ws://127.0.0.1:<port>/api/v1/ingest/local`, with a local token), using the same frame formats. Local ingest never listens on the LAN.
