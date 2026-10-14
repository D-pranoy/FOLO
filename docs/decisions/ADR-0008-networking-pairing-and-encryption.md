# ADR-0008: Networking, pairing and encryption

Status: Accepted
Date: 2026-10-13

## Context

- Only the controller may open ports ([ADR-0003](ADR-0003-system-roles.md)).
- Sources are fully managed by their controller, including pack installs and update approvals, so the link is high-value.
- The UI is served over plain HTTP ([ADR-0009](ADR-0009-client-access.md)).

Paseo's daemon/client design was a reference: pairing offers carrying public keys, application-layer end-to-end encryption, QR or link as the trust anchor. SPAKE2 implementations in Rust and Python are unaudited.

## Decision

- **D1.** The controller listens on **one port** (default 8777) for the API, UI, source links and ingest. Sources make outbound connections only.
- **D2.** Each node has a static X25519 identity. The node id is derived from the public key.
- **D3.** **Pairing by invite** (primary):
  - The controller issues a one-time invite (QR or link): addresses, static public key, a 128-bit secret, a 10-minute expiry.
  - The source proves possession of the secret inside the handshake.
- **D4.** **Pairing by join request:**
  - A source that found a controller through mDNS sends a join request.
  - Both sides show a 6-digit verification code derived from the handshake.
  - An admin approves on the controller.
- **D5.** The source link is a WebSocket carrying **Noise IK** (`Noise_IK_25519_ChaChaPoly_BLAKE2s`), encrypted end to end over the plain HTTP transport ([source link](../specs/source-link.md)).
- **D6.** A source has **one owner** controller (admin rights) and optional **consumer** controllers (surfaces only).
- **D7.** Recovery (`folo pair reset`) requires being at the machine.
- **D8.** Controllers advertise `_folo._tcp` over mDNS. Source browsing is optional and can be turned off.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| SPAKE2 with short typed codes | Typing-friendly | Unaudited libraries; invites and verification codes cover the need |
| Mutual TLS on a second port | Standard | A second port; certificate handling; Noise fits one port |
| Signed messages without encryption | Simpler | No confidentiality for a high-value link |
| Sources listen and controllers dial | Simpler discovery | Opens ports on every PC |

## Consequences

- One Noise implementation, in the Rust node (`snow`). The Python engine stays loopback-only.
- Formats: [pairing and connect](../specs/pairing-and-connect.md).
