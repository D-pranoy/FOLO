# ADR-0013: Hue provider, managed configurations and zone sync

Status: Accepted
Date: 2026-10-13

## Context

- Hue lights stream through the bridge's Entertainment API: DTLS 1.2 PSK over UDP 2100, HueStream v2 packets of up to 20 channels.
- A bridge allows **one active stream**.
- Entertainment configurations can be created, updated and deleted through CLIP v2 (confirmed in Q42.HueApi's client and tests).
- There is no Rust Hue Entertainment crate. The `openssl` crate provides DTLS with a PSK client callback, and has been proven against real bridges.
- Reverse engineering Hue Sync isn't needed: open-source implementations document the protocol.

## Decision

- **D1.** **Streaming:**
  - HueStream v2 in RGB, every channel in every packet, about 50 Hz.
  - Keepalive within 10 s (resend at least every 5 s while idle).
  - Mapping by `channel_id`.
- **D2.** **DTLS:** the `openssl` crate (vendored OpenSSL on Windows), `TLS_PSK_WITH_AES_128_GCM_SHA256`. Identity is the `hue-application-id`, falling back to the username. Fallback library: the pure-Rust `rtc-dtls`.
- **D3.** **REST:** reqwest with rustls and a custom verifier. The chain must lead to the bundled Signify roots, and the CN must equal the bridge id.
- **D4.** **Pairing** through the link button, with `generateclientkey`.
- **D5.** **Never take over** another app's active stream automatically; offer it in the UI.
- **D6.** **One FOLO-managed entertainment configuration per bridge**, holding the Hue lights of all active FOLO zones, so several zones can run at once. A per-bridge setting streams to the linked Hue area instead.
- **D7.** **Zone sync** per zone: Off, Import from Hue, Export to Hue, or Two-way. Two-way is the default for linked zones, with conflicts asking the user. Hue changes are detected through the event stream.
- **D8.** Gradient lights are line targets, with one element per channel.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Drive Hue through Hyperion | No new code | Not direct |
| REST light commands only | Simple | Roughly 10 commands/s: too slow |
| Direct Zigbee or Bluetooth control | No bridge | Extra hardware or slow; bulbs leave the Hue app |
| Stream only to linked Hue areas | No extra configuration | One FOLO zone per bridge |

## Consequences

- FOLO creates and updates an entertainment configuration on users' bridges (named `FOLO`).
- A fake bridge (REST double plus `openssl s_server` with DTLS-PSK) is used in CI.
- Details: [Hue](../integrations/hue.md).
