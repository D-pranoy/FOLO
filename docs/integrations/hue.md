# Philips Hue

Status: Accepted · Implementation: Partial (RB-13 steps 1–6, 9 and 10: discovery, trust, pairing, streaming, ownership and administration; managed configurations and zone sync are still to come)

Decided in [ADR-0013](../decisions/ADR-0013-hue-provider-and-zone-sync.md). Administration scope is in [ADR-0014](../decisions/ADR-0014-device-administration-scope.md).

## How Hue reaches the lights

1. **Client → Bridge, setup over HTTPS:** discovery, pairing, and the CLIP v2 REST API for resources and entertainment configurations.
2. **Client → Bridge, live frames:** DTLS 1.2 over UDP port 2100 (the Entertainment API).
3. **Bridge → lights over Zigbee:** standard lighting clusters for normal control. Entertainment uses Signify's undocumented, encrypted Zigbee messages. FOLO never touches this layer.

FOLO implements layers 1 and 2. Hue Sync and other apps use the same API.

## Protocol facts

Sources: the Hyperion.NG, diyHue, Q42.HueApi, aiohue and hue-entertainment-pykit source code; music-assistant/hue-entertainment; iotech.blog. Signify's official entertainment documentation needs a developer login.

| Step | Detail |
|---|---|
| Discovery | mDNS `_hue._tcp.local.` (most reliable), fallback `https://discovery.meethue.com/`, manual IP |
| Verify | Unauthenticated `GET /api/config` returns `bridgeid`, `modelid` (BSB002 square v2, BSB003 Bridge Pro), `swversion` and `apiversion`. Entertainment needs `apiversion` ≥ 1.22. |
| TLS | Certificates chain to Signify roots: "root-bridge" (valid 2017–2038) and "Hue Root CA 01" (valid 2025–2050, Bridge Pro). The certificate CN is the bridge id. Connect by IP, so verify the chain and require CN = bridge id (case-insensitive) instead of the hostname. |
| Pairing | After the link button is pressed: `POST /api {"devicetype":"folo#<host>","generateclientkey":true}` returns `username` (the app key) and `clientkey` (16 bytes, hex). Error 101 means the button wasn't pressed. `devicetype` is limited to 20 characters for the app part and 19 for the device part. |
| PSK identity | `GET /auth/v1` with the `hue-application-key` header returns a `hue-application-id` header. Use it as the DTLS PSK identity (fall back to the username), and use it to recognise our own stream in `active_streamer`. |
| Areas | `GET /clip/v2/resource/entertainment_configuration` lists configurations: `metadata.name`, `configuration_type` (screen, monitor, music, 3dspace, other), `status`, `active_streamer`, `channels[{channel_id, position{x,y,z}, members[{service, index}]}]`, `locations.service_locations[{service, positions[1–2], equalization_factor}]` |
| Start | `PUT …/entertainment_configuration/<id> {"action":"start"}`, retried 3 times 500 ms apart. Starting doesn't change an active stream's owner; taking over means stop, then start. |
| DTLS | DTLS 1.2, PSK, `TLS_PSK_WITH_AES_128_GCM_SHA256`, key = the hex-decoded clientkey. Hand-shake immediately after start. Retransmit timers 600–1000 ms, up to 5 attempts. Send close_notify before stop. |
| Packets (v2) | `"HueStream"` · `02 00` · sequence · `00 00` · colour space (`00` RGB, `01` xy+brightness) · `00` · the area UUID as 36 lowercase ASCII characters · up to **20** records of `channel_id (u8), R, G, B (u16 big-endian each)` |
| Encoding | 8-bit to 16-bit by byte duplication (255 → 0xFFFF). **Every packet carries every channel** (diyHue loses sync otherwise). The sequence increments modulo 256. |
| Timing | Send at about 50 Hz; the bridge drives lights at about 25 Hz. The stream closes after **10 s without data**, so resend the last frame at least every 5 s while idle. |
| Stop | close_notify, then `PUT {"action":"stop"}` (retried) |
| Limits | One active entertainment stream per bridge. The Hue app allows about 10 lights per area. REST is limited to roughly 10 light commands/s and 3 concurrent requests; 429 means back off. |

**Unresolved differences between implementations:**
- the PSK identity: Q42 and diyHue use the username; Hyperion and pykit use the application id
- the meaning of byte 15

FOLO uses the application id with a username fallback, and the framing above, which every implementation accepts.

## Entertainment configuration CRUD

Confirmed in Q42.HueApi's client and tests:
- **Create:** `POST /clip/v2/resource/entertainment_configuration` with `metadata.name`, `configuration_type` and `locations.service_locations` (entertainment service id, 1–2 positions, optional `equalization_factor`). The bridge derives the channels.
- **Update:** `PUT` with new positions; the channels change accordingly.
- **Delete:** `DELETE`.

## FOLO provider design

- **Targets:**
  - A single-channel light becomes a **point** target at its channel position.
  - A gradient light becomes a **line** target with one element per channel, ordered by member index.
  - Mapping is strictly by `channel_id`, never by array position.
- **Colour:** FOLO sends RGB after its own adapters (gamma, calibration, flash limiter). The bridge handles gamut mapping.
- **DTLS:** the `openssl` crate (DTLS client, PSK client callback), with OpenSSL vendored on Windows. Fallback: the pure-Rust `rtc-dtls`.
- **Sessions:**
  - If another app owns the active stream, the UI shows "in use by another app" and offers **take over**. FOLO never takes over automatically.
  - On errors, FOLO tears down, backs off and starts again.

## Managed entertainment configurations

FOLO may run several zones with Hue lights at once, but a bridge allows only one active stream.

- For each bridge, FOLO maintains **one managed entertainment configuration** named `FOLO`, holding the Hue lights of every currently active FOLO zone. FOLO does its own projection, so the positions stored in it don't affect FOLO's output.
- The configuration is recreated when the set of active zones on that bridge changes. This briefly stops and restarts the stream.
- Hue's per-configuration channel limit applies. FOLO warns before a zone activation would exceed it.
- A per-bridge setting can switch to streaming to the **linked** Hue area of the single active zone instead. Then only one FOLO zone per bridge can be active.

## Zone sync

Each FOLO zone with Hue lights has a sync mode:

| Mode | Behaviour |
|---|---|
| Off | No link |
| Import from Hue | A Hue area becomes or refreshes a FOLO zone: lights become targets at converted positions; the zone type comes from `configuration_type` |
| Export to Hue | The FOLO zone's Hue lights become a Hue area that FOLO creates or updates |
| Two-way (default for linked zones) | Changes on either side are applied to the other. Conflicts ask the user. Hue's changes are detected through the bridge's event stream. |

- **Types map** as follows: screen/monitor ↔ TV/Screen; music ↔ Music; 3dspace ↔ Room; other ↔ Custom or Desk.
- **Positions** convert between metres and Hue's [-1, 1] with the zone bounds ([spatial model](../architecture/spatial-model.md#coordinate-conversion-with-hue)). Gradient lights use 1–2 positions.
- **Only Hue lights sync.** Other targets in the zone stay FOLO-only.

## Basic administration

- bridge information and link-button pairing
- light list (rename, identify)
- read-only rooms and zones
- entertainment configuration create, edit and delete, and sync
- bridge update status
- the "in use by another app" view

Scenes, automations and accessories stay in the Hue app.

## Implementation notes

What `crates/folo-provider-hue` does, where this page leaves a choice.

- **Pieces.** `HueKind` is the kind a node registers; `HueProvider` is an instance. Outside the provider traits the crate offers `bridge::probe` (ask an address who it is, and whether its certificate is believed) and `pairing::pair` (the link-button flow). Their results belong to the node: the credentials go to the secrets store (`app_key` and `client_key`; the application id is read from `/auth/v1` at every connection and isn't stored), and a confirmed certificate goes back in through `HueOptions::pins`. The node keeps the confirmed certificates in `secrets.json` (`pinned_certificates`, by lowercase bridge id; `SecretsPins` in `folo-node`) and reads the credentials from `providers.<id>`; nothing in the node writes the credentials yet, because the routes for pairing arrive with RB-13 steps 7 and 8.
- **Trust.**
  - The REST client is `reqwest` over rustls (ring) with a verifier of our own ([ADR-0013](../decisions/ADR-0013-hue-provider-and-zone-sync.md) D3): the chain is checked with OpenSSL against the bundled Signify roots and the common name against the bridge id, the handshake signatures by rustls.
  - The roots are `root-bridge` (fingerprint `F0:BD:8E:65:…:8A:CF`) and `Hue Root CA 01` (`D8:B8:94:48:…:28:B8`), in `src/signify_roots.pem`, and a test pins their SHA-256 fingerprints.
  - A bridge whose certificate is named for it but doesn't chain to a Signify root (older firmware) is offered for confirmation with its fingerprint. After the person confirms, the fingerprint is pinned and the name check still applies. A certificate for another name is never offered.
- **Failed trust.** A certificate that isn't believed is `Refused` (not `Unreachable`), so the instance shows a stable error instead of retrying as if the bridge were down. The chain check uses the intermediates the bridge sends. A pinned bridge that later presents a Signify chain is believed by the chain.
- **Checks.** A bridge is used when its `modelid` is `BSB002` or `BSB003` and its `apiversion` is 1.22 or later.
- **Discovery.** mDNS first; Signify's service only when mDNS finds nothing; the manual address goes through `probe`. Tests inject the discovery URL and its root, so none reaches the internet.
- **Targets.** A target's id is `<configuration id>/<entertainment service id>`: the same light is a different target in each configuration it is part of, because each has its own channel for it. Targets declare RGB, streaming and `max_rate_hz` 50. `latency_ms` is left at 0 until live verification measures it. Layouts are not offered: converting the bridge's positions needs the zone's bounds, which belong to zone sync.
- **Streaming.**
  - Until managed configurations arrive, `managed` and `linked` behave the same: the stream goes to the configuration the started targets belong to, and a target of another configuration is refused (`Refused`, "one stream") while one runs.
  - The stream begins with the first target started and ends with the last released: `close_notify`, then the configuration's `stop`.
  - A packet holds every channel of the configuration. A channel no started target drives is black. More than 20 channels are refused.
  - A stream that fails (`Unreachable`) is closed, and the configuration is stopped when the connection is back.
- **DTLS retransmits.** The `openssl` crate has no datagram socket BIO, so OpenSSL's own retransmit timers don't run. A handshake that gets no answer within a random 600–1000 ms starts over on the same socket, up to five times.
- **Ownership.** `start` reads the configurations first. A stream of another application anywhere on the bridge (`active_streamer` is not our application id) is reported as "in use by another app" and nothing is touched. A stream of ours left behind is stopped and started again. Taking over is the administration action `entertainment_take_over`, and only when asked. Every other stop FOLO sends (a release, a failed handshake, the cleanup after a lost stream) first reads the configuration and stops it only if our application id is the streamer, so another app's stream is never ended by accident.
- **Administration.** The actions of the provider's `admin` entry point. A read says what a change would change ([ADR-0014](../decisions/ADR-0014-device-administration-scope.md) D3); the names are flat, without slashes.

  | Action | Read | Change |
  |---|---|---|
  | `bridge` | model, versions, address | |
  | `lights` | the lights, whether gradient and whether they can stream | |
  | `light_rename` `{id, name}` | the rename | renames the light's device |
  | `light_identify` `{id}` | what would blink | makes it blink |
  | `rooms` | rooms and zones with their lights | |
  | `entertainment` | the configurations and `in_use_by_another_app` | |
  | `entertainment_create` `{name, configuration_type?, lights[{light or service, positions?, equalization_factor?}]}` | the new configuration | creates it |
  | `entertainment_update` `{id, name?, configuration_type?, lights?}` | the change | applies it |
  | `entertainment_delete` `{id}` | what goes | deletes it |
  | `entertainment_take_over` | whose stream would stop | stops it |
  | `update_status` | the bridge's software update state | |

  A configuration FOLO streams to isn't edited or deleted. A request that is wrong answers `provider_error` with "invalid request", because the admin errors have no bad-request kind.
- **Tests.** The fake bridge (`tests/support/`) serves the old `/api`, `/auth/v1`, the CLIP v2 resources, configuration CRUD and the event stream over HTTPS with a test authority, and a DTLS-PSK endpoint that decrypts what it is sent. `tests/openssl_server.rs` also runs the run book's `openssl s_server`; it is skipped on Windows and without the command unless `FOLO_OPENSSL_TESTS=1`, which CI's Ubuntu job sets.

## Licences of the references

| Source | Licence |
|---|---|
| Hyperion.NG | MIT |
| Q42.HueApi | MIT. Its copy of Signify's CLIP documentation must not be copied. |
| diyHue | Apache-2.0, except its `Lights/` folder (CC BY-NC 4.0), which must never be copied |
| aiohue, music-assistant/hue-entertainment | Apache-2.0 |
| hue-entertainment-pykit | MIT |

FOLO's Hue code is written independently. These projects are references only.
