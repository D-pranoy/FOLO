# Pairing and connect codes

Status: Draft · Implementation: Partial (the node's identity)

Decided in [ADR-0008](../decisions/ADR-0008-networking-pairing-and-encryption.md) and [ADR-0009](../decisions/ADR-0009-client-access.md). Every code here works as a link, as a QR code, or as pasted text.

## Identity

- Each node has a static **X25519** key pair (for Noise), stored in `secrets.json` with owner-only permissions.
- **Node id:** the lowercase base32 encoding ([RFC 4648](https://www.rfc-editor.org/rfc/rfc4648#section-6): `a`–`z` and `2`–`7`, no padding) of the first 10 bytes of `BLAKE2s(public_key)` (BLAKE2s-256), giving 16 characters.
- **Fingerprint shown to users:** the node id in four groups of four, for example `k3xq-2mfa-q7tc-4pzr`.

## Source invite

Issued by a controller (`POST /api/v1/sources/invites`). Single use, valid for 10 minutes by default. A handover invite may allow several uses for a short period.

```
folo://join?v=1&c=<controller id>&pk=<controller public key, base64url>&a=<addr>[,<addr>…]&s=<secret, base64url, 16 bytes>&e=<expiry, unix seconds>&n=<controller name, url-encoded>
```

- Accepted as a QR scan or paste in the source's UI, by `folo join <code>`, or through `FOLO_JOIN=<code>`.
- **Proof:** the source proves possession of `s` inside the Noise handshake ([source link](source-link.md#handshake)). `s` itself never goes over the wire.
- **Treat invites like passwords.** They are shown only to admins.

## Join request

- A source that found a controller through mDNS (whose TXT record includes `pk`) sends `purpose: join_request`.
- The controller lists the request under Sources → Requests. **Both screens show the same 6-digit verification code.**
- An admin approves or rejects. Requests expire after 10 minutes.

## Client connect code

Issued by a controller for one client (`POST /api/v1/clients`, admin scope), with a name and a scope:

```
folo://connect?v=1&n=<controller name>&a=<addr>[,<addr>…]&k=<client key>&f=<controller id>
```

- The client stores the host entry: name, addresses, key and controller id.
- The client key is sent as a bearer token over LAN HTTP. The controller id is kept so the connection can be upgraded to encrypted transport later.
- **Client keys:** 32 random bytes, base64url encoded, prefixed with `folo_c_`. Stored hashed on the controller. Revocable per client. Scopes: `control` and `admin`.
- **Master admin key:** shown on first start in the logs and the local UI, and by `folo token show`. Rotated with `folo token rotate`. Meant for the CLI and recovery.

## Approval-based client pairing

For clients that can't scan a code, such as the Home Assistant integration:
1. The client calls `POST /api/v1/auth/requests` with its name and a random request id, and shows the id to its user.
2. An admin approves the matching request in FOLO's UI.
3. The client's pending call returns its key. Requests expire after 3 minutes.
