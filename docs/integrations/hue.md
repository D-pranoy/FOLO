# Philips Hue

Status: Accepted · Implementation: Not started

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

## Licences of the references

| Source | Licence |
|---|---|
| Hyperion.NG | MIT |
| Q42.HueApi | MIT. Its copy of Signify's CLIP documentation must not be copied. |
| diyHue | Apache-2.0, except its `Lights/` folder (CC BY-NC 4.0), which must never be copied |
| aiohue, music-assistant/hue-entertainment | Apache-2.0 |
| hue-entertainment-pykit | MIT |

FOLO's Hue code is written independently. These projects are references only.
