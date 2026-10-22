# Security

Status: Accepted · Implementation: Not started

## Assets

- Control of lights: physical safety and annoyance.
- Device credentials: Hue app keys and client keys, WLED PINs, Hyperion tokens.
- Remote code paths: pack installs, updates.
- Screen and audio content on source machines: privacy.

## Threats considered

| Threat | Control |
|---|---|
| Unauthorised LAN client | Per-client API keys; LAN-only client filter; failed-login backoff; audit log |
| DNS rebinding from a browser | Host-header allowlist (loopback names plus configured LAN names and IPs) |
| Cross-site requests | JSON-only mutating requests; Origin check; HttpOnly, SameSite=Strict session cookies |
| Sniffing on the LAN | **Source links are Noise-encrypted.** UI traffic is plain HTTP by decision ([ADR-0009](../decisions/ADR-0009-client-access.md)). Anyone who can sniff the LAN can capture a client key. Optional HTTPS is a later option. |
| Rogue source or impersonated controller | Pinned static keys after pairing; one-time, expiring invites; join requests require a verification code and admin approval |
| A local program posing as the effect engine | The node starts its engine with a launch token in the engine's environment, and its loopback source link accepts only that token ([ADR-0032](../decisions/ADR-0032-local-source-link.md)) |
| Malicious content pack | Data-only packs, validated and clamped; size limits; no code execution in v1 ([ADR-0021](../decisions/ADR-0021-content-packs.md)) |
| Tampered update | Signatures verified against a public key built into FOLO; SHA-256 checks; approvals; provenance attestations ([updates](updates.md)) |
| Compromised build pipeline | Pinned actions, least-privilege workflow permissions, release environment with a required reviewer, Dependabot, CodeQL, cargo-deny ([ADR-0019](../decisions/ADR-0019-ci-cd-versioning-and-releases.md)) |
| Photosensitive harm | A per-zone flash limiter, off by default, and photosensitive epilepsy warnings on first run, in zone settings, in Settings and on fast-flashing effects ([ADR-0033](../decisions/ADR-0033-flash-limiter-off-by-default.md), [capabilities](capabilities.md#flash-limiter)) |
| Privacy of captured media | Capture on demand only; visible indicator; full frames and raw audio never stored or sent ([display capture](../integrations/display-capture.md#latency-and-privacy)) |

## Rules

- **Remote access is off by default.** When on, client IPs outside these ranges are rejected:
  - loopback
  - private IPv4: 10/8, 172.16/12, 192.168/16
  - link-local
  - unique-local IPv6: fc00::/7
- **Loopback clients need no key.** The Host check still applies.
- **Static UI files load without a key; every API and WebSocket call requires one.** The exceptions are `/api/v1/health` and the pairing endpoints, which use their own proofs.
- **Secrets** live in a separate file with owner-only permissions. Secrets are excluded from every API response, log and export.
- **Device certificates:**
  - Hue bridges are verified against the bundled Signify root certificates, with CN = bridge id.
  - Non-Signify bridges (for example diyHue) use trust-on-first-use pinning after the user confirms.
- **Client key scopes:** `control` (everyday use) and `admin` (configuration, pairing, updates, packs). Updates and pairing need `admin`.
