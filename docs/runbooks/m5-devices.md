# M5 Devices

Status: Accepted · Implementation: Partial (RB-13 steps 1–6, 9 and 10)

## RB-12: WLED provider

**Goal:** WLED driven directly, with capabilities, segments and basic administration.
**Depends on:** RB-08 step 6 (the [provider interface](../architecture/providers.md)).

1. **Fake first:** a UDP receiver that parses DDP (push semantics, offsets, data types) and DNRGB(W) (timeout byte, start index), plus fixture JSON for `/json/info`, `/json/state` and `/json/cfg` covering RGB, RGBW, CCT, analog, on/off and 2D.
2. **Discovery:** mDNS `_wled._tcp`; manual entry.
3. **Describe** per [WLED](../integrations/wled.md#capabilities):
   - capabilities from `lc`, `seglc` and bus types
   - the strip as a line target; segments as sub-targets, with warnings for `grp`/`spc`
   - a matrix as a grid; PWM and relay outputs as point targets
4. **DDP sender:**
   - header `0x40` with push on the last packet, sequence 1–15
   - RGB or RGBW data types, byte offsets, at most 1440 data bytes per packet
   - warn on a non-default DMX start address
5. **DNRGB(W) sender** as an option, with its timeout byte.
6. **Rate and output:** 60 fps by default; refresh at least once a second while static. Gamma applied by FOLO. White channel sent only when the device's auto-white mode honours it. Main-segment-only handled through `liveseg`.
7. **Release:** DNRGB with timeout 0, or let the timeout run out.
8. **Administration:** info, on/off, brightness, rename, **segment create/edit/delete**, realtime-settings check and fix (PIN where needed), reboot, open the device UI. Each action states its change first.
9. **Live verification** per the [testing policy](../process/testing-policy.md).

**Done when:** the fake suites are green and live verification has passed.

## RB-13: Hue provider and zone sync

**Goal:** Hue entertainment streaming, managed configurations, zone sync and basic administration.
**Depends on:** RB-08 step 6 (the [provider interface](../architecture/providers.md)).
**Status:** steps 1–6, 9 and 10 are done in `crates/folo-provider-hue` (see the [implementation notes](../integrations/hue.md#implementation-notes)); steps 7 and 8, the sync tests of step 10, and step 11 remain.

1. **Fake bridge first:**
   - a REST double: `/api/config`, `/api` pairing (link-button states), `/auth/v1`, CLIP v2 resources and entertainment configuration CRUD, the event stream
   - a DTLS endpoint: `openssl s_server -dtls1_2 -nocert -psk <hex> -psk_identity <id> -cipher PSK-AES128-GCM-SHA256` on Linux CI runners, with the decrypted payload checked by the test
2. **Discovery and trust:**
   - zeroconf `_hue._tcp`, discovery.meethue.com fallback, manual entry
   - `/api/config` checks (model; `apiversion` ≥ 1.22)
   - bundled Signify roots; CN = bridge id
   - trust-on-first-use for non-Signify bridges, after confirmation
3. **Pairing:** a link-button countdown; poll `POST /api` with `generateclientkey`; store the username and clientkey as secrets; read the application id from `/auth/v1`.
4. **Model:** read entertainment configurations, entertainment services, lights and devices. Describe point targets for single-channel lights and line targets for gradient lights, mapped by `channel_id`.
5. **Streaming:**
   - DTLS through the `openssl` crate (vendored on Windows): PSK identity = application id (fallback: username), key = hex-decoded clientkey, retransmits 600–1000 ms, 5 attempts
   - HueStream v2 RGB encoder: every channel in every packet, at most 20 channels per packet, sequence modulo 256, byte duplication
   - 50 Hz; keepalive at least every 5 s; close_notify, then stop
   - backoff on errors
6. **Ownership:** detect another app's active stream, show "in use by another app", and offer take-over (stop, then start) only after the user confirms.
7. **Managed configurations:**
   - per bridge, create or update a `FOLO` entertainment configuration holding the Hue lights of the active zones
   - recreate it when the active set changes; warn when channel limits would be exceeded
   - the per-bridge "linked" mode as the alternative
8. **Zone sync** per [Hue](../integrations/hue.md#zone-sync):
   - Off, Import, Export, Two-way
   - type mapping; metre ↔ normalised conversion; gradient positions
   - change detection through the event stream; conflict prompts
9. **Administration:** bridge info, lights (rename, identify), read-only rooms, entertainment configuration CRUD, bridge update status.
10. **Golden bytes** for the encoder; tests for mapping, conversion and sync.
11. **Live verification** per the [testing policy](../process/testing-policy.md): pairing, a test area at low brightness, identify sweeps, gradient segments, managed configuration lifecycle, and two-way sync with the Hue app.

**Done when:** the fake bridge suite is green and live verification has passed.
