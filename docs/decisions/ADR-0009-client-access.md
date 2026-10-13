# ADR-0009: Client access

Status: Accepted
Date: 2026-10-13

## Context

Users manage controllers from the desktop app, browsers and, later, an Android app, and may run several controllers. Remote access is restricted to the LAN. Plain HTTP with an API key was chosen for client access. The future Android client pairs by scanning a QR code or pasting a key.

## Decision

- **D1.** Remote access is **off by default**. When on, it is **LAN-only**: clients outside loopback, RFC 1918, link-local and fc00::/7 are rejected. The Host-header allowlist stays.
- **D2.** Clients authenticate with **per-client, revocable API keys** over **plain HTTP** (bearer header; HttpOnly, SameSite=Strict cookie for browsers). Scopes are `control` and `admin`. A master admin key exists for the CLI and recovery.
- **D3.** Keys are issued through a **connect code** (QR, link or paste) holding the controller's name, addresses, the key and the controller id. An approval-based request flow serves clients that can't scan codes (Home Assistant).
- **D4.** **Loopback clients need no key.** Static UI files load without a key; the API requires one.
- **D5.** Clients keep a **host list** with several addresses per controller, and switch the active controller. The desktop app and the Android app show each controller's own UI, with a built-in Hosts screen.
- **D6.** The Android client is a future Tauri v2 mobile build. The UI, API, codes and CORS rules are designed for it now.
- **D7.** Optional HTTPS for client access is a later option. The known risk of plain HTTP (a client key can be sniffed on the LAN) is documented in [security](../architecture/security.md).

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| One shared API key | Simplest | No per-device revocation |
| HTTPS by default | Confidentiality | Certificate friction on the LAN; contrary to the decision |
| Clients bundle the UI and talk to any controller | Works offline | Version skew between UI and controllers |

## Consequences

The Home Assistant integration uses the approval flow. Formats: [pairing and connect](../specs/pairing-and-connect.md).
