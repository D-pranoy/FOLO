# ADR-0032: The effect engine's link to its node

Status: Proposed
Date: 2026-10-15

## Context

The effect engine (`folo-source`) is a child of a node. The documents fix several things about the link between them:
- It publishes surfaces and signals to the node over loopback ([overview](../architecture/overview.md), RB-04 step 7).
- It gets its settings from the node, "on the command line or over a loopback handshake" ([RB-03](../runbooks/m2-engine.md#rb-03-effect-engine-restructure) step 5).
- The node checks its heartbeat and restarts it ([control plane](../architecture/control-plane.md#supervision)).
- The node will serve a loopback source-link endpoint for its local source, and encryption for remote sources comes later ([RB-07](../runbooks/m3-controller-core.md#rb-07-controller-api) step 6).
- The source link between machines is a WebSocket carrying Noise IK ([ADR-0008](ADR-0008-networking-pairing-and-encryption.md) D5). Noise lives only in the Rust node, and the Python engine stays loopback-only.

They don't say:
- how the engine reaches the endpoint and proves it's the node's own engine, since a loopback port is open to every local program
- what takes the Noise handshake's place on loopback
- how often the heartbeat comes
- what the engine does when its node is gone
- what the LZ4 flag of a frame means, as a block or as a frame
- which Python libraries the engine uses for this

Loopback API clients need no key ([security](../architecture/security.md#rules)), but local ingest already uses a local token, and frames on the source link drive lights.

## Decision

- **D1.** **Transport:**
  - The engine dials its node at `ws://127.0.0.1:<port>/api/v1/source-link`, with the `folo.source-link.v1` subprotocol.
  - On loopback the source-link messages aren't wrapped in Noise: each WebSocket binary message is one message (its type byte and payload).
  - The handshake payloads are JSON in the first message each way.
- **D2.** **The launch token:**
  - The node starts the engine with the link's URL and a random 256-bit launch token (base64url) in its environment, `FOLO_SOURCE_LINK` and `FOLO_SOURCE_TOKEN`. They never go on its command line, which other users can read.
  - The engine's handshake payload has the purpose `{kind: local, token}`.
  - The node compares the token in constant time, accepts one engine connection at a time, and closes any other.
- **D3.** **Settings:** after the node's handshake response comes `config.apply`, as for any source. That is RB-03 step 5's loopback handshake. Until it arrives, the engine renders nothing.
- **D4.** **Heartbeat:** the engine sends a `status` notification at least once a second. After 5 s without any message from it, the node treats it as hung.
- **D5.** **Losing the node:** the engine reconnects with the source link's backoff. After 30 s without its node it exits, so an orphaned engine doesn't keep capturing audio.
- **D6.** **LZ4:** a frame's LZ4 flag means its payload is an LZ4 *frame* (the LZ4 frame format, which records the content size), on every source link. A sender compresses only when that makes the payload smaller.
- **D7.** **Libraries:** the engine runs the link on a thread of its own with the `websockets` package (asyncio), and compresses with the `lz4` package. Both are BSD-licensed.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| The engine's stdin and stdout | Private to the pair; no port or token | A second transport beside the loopback endpoint RB-07 plans; binary framing and flow control on pipes; stderr already carries its logs |
| Noise on loopback too | One protocol everywhere | A Python Noise implementation, which ADR-0008 keeps out of the engine |
| No token, as for loopback API clients | Simplest | Any local program could pose as the engine and drive the lights |
| The token on the command line | Simple | Other users can read command lines |
| LZ4 block format | A smaller header | No content size, so a receiver can't bound its buffer before decompressing |

## Consequences

- **Contract:** the source link gains the `local` purpose with its token (an additive change: version 1.2).
- **Documents:** [source link](../specs/source-link.md) (the local link and LZ4), [control plane](../architecture/control-plane.md#supervision) (the launch and the heartbeat) and [security](../architecture/security.md) (the threat) describe it. RB-04 step 7 names the libraries, RB-06 step 6 starts the engine this way, and RB-07 step 6 checks the token.
- **RB-04 step 7** implements the engine's side against a fake node in tests. The node's side comes with RB-06 and RB-07.
- **Remote sources:** a source node's own engine uses the same local link to that node, which forwards over Noise to its controllers.
