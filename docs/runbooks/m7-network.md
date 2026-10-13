# M7 Network

Status: Accepted · Implementation: Not started

## RB-15: Client access

**Goal:** LAN-only remote access with per-client keys, connect codes and several controllers per client.
**Depends on:** RB-07, RB-11.

1. **Remote access setting:** off by default. When on, bind the configured interface, allow private client ranges only (loopback, RFC 1918, link-local, fc00::/7) and extend the Host allowlist with LAN names.
2. **Client keys** per [pairing and connect](../specs/pairing-and-connect.md#client-connect-code):
   - `folo_c_` prefix, stored hashed, `control` and `admin` scopes, revocation
   - the master admin key (`folo token show|rotate`)
3. **Connect codes:** "Add a device" issues a key and shows the QR code and the link.
4. **Approval-based pairing:** `POST /auth/requests` with a request id; admin approval in the UI; expiry after 3 minutes.
5. **Browser sessions:** login form → HttpOnly, SameSite=Strict cookie; Origin checks; failed-login backoff; an audit log of admin actions.
6. **mDNS:** advertise `_folo._tcp` with `id`, `pk`, `api` and `name`.
7. **CORS** rules admitting the desktop app's and the future Android app's origins.
8. **Tests:** IP filter, Host check, scopes, revocation, cookie and Origin behaviour, approval-flow expiry.

**Done when:** a second LAN machine can connect by connect code, non-LAN addresses are rejected, and the tests are green.

## RB-16: Source pairing and link

**Goal:** remote sources joining by invite or join request, over Noise-encrypted links, managed centrally.
**Depends on:** RB-06, RB-07, RB-08.

1. **Invites:** `POST /sources/invites` issues one-time codes ([format](../specs/pairing-and-connect.md#source-invite)); multi-use handover invites; expiry.
2. **Source side:**
   - `folo join <code>`, `FOLO_JOIN` and the UI (scan or paste)
   - `role=source` binds the local API to loopback only
   - reconnect with backoff; addresses refreshed from mDNS
3. **Handshake:** `Noise_IK_25519_ChaChaPoly_BLAKE2s` through `snow` over the WebSocket. The invite proof is `HMAC(secret, handshake_hash)`. Payloads carry versions and node information ([source link](../specs/source-link.md#handshake)).
4. **Join requests:** source-side mDNS browsing (can be turned off); a pending list on the controller; a 6-digit verification code on both screens; approve or reject; expiry.
5. **Messages:**
   - JSON-RPC methods and notifications per the spec
   - surface frames: newest wins, staleness timeout
   - signal frames for activity
6. **Central management:**
   - per-source configuration (inputs, assigned surfaces, scenes, publish settings), pushed by revision; the last applied configuration is kept
   - pack distribution
   - owner and consumer controllers
7. **Role switching:**
   - "Join another controller" in the UI: dormant configuration, released outputs, the handover prompt for this controller's own sources, the scene-upload offer
   - "Leave controller", and release by the owner
8. **External surface ingest:** `/api/v1/ingest/surfaces` with scoped tokens. **Local ingest** on sources, loopback only.
9. **Recovery:** `folo pair reset`, which must be run at the machine.
10. **Tests:**
    - pairing paths: invite, join request, wrong proof, replayed invite, expiry
    - a key-pinning violation
    - reconnect; switching and crossfade
    - ingest authentication
    - a three-node integration test (two sources and one controller) in CI

**Done when:** the multi-node integration test is green and a source on a second machine drives a zone end to end.

## RB-17: OTA updates

**Goal:** signed, approved updates for every install type.
**Depends on:** RB-06, RB-16, RB-23 (manifests).

1. **Update service:**
   - install-type detection from the packaging marker
   - checks every 24 h with jitter, and on demand, against GitHub Releases (`folo-update.json`)
   - stable and beta channels
2. **Verification:** minisign signatures through `minisign-verify` against the public key built into FOLO, plus the SHA-256 from the manifest. There is no option to skip either.
3. **Approvals:**
   - the controller's update table for every node
   - policy: manual by default, automatic patches, maintenance windows
   - order enforcement: controller first when the protocol changes
   - admin scope required
4. **Apply** per [updates](../architecture/updates.md#apply-step-by-install-type):
   - **Windows installer:** detached silent run with `/UPDATE`
   - **Archives:** A/B version folders behind a launcher pointer
   - **AppImage:** atomic replacement, then re-launch
   - **deb, rpm, AUR, Docker:** notify only, with instructions
5. **Health and rollback:** report healthy within 120 s; archives roll back automatically; installers notify.
6. **Desktop app:** progress UI for the local node; the app updates through the same flow.
7. **Tests:**
   - valid, invalid and tampered signatures
   - each apply strategy in temporary folders
   - an end-to-end run against a fake release server
   - a dry-run update matrix in CI

**Done when:** update dry-runs pass for every install type and tampered artifacts are rejected.

## RB-18: Home Assistant readiness

**Goal:** everything a future Home Assistant integration needs, without the integration itself.
**Depends on:** RB-15, RB-16.

1. **API checks:** stable controller and zone ids; mDNS TXT `id` and `api`; full-state WebSocket push; the effect catalog with parameter schemas; approval-based pairing; scoped keys; OTA summary and release URL.
2. **Client library:** `python-folo`, async (aiohttp), typed, accepting an injected session, generated from the OpenAPI contract plus hand-written live-channel support.
3. **Publishing:** to PyPI from CI on tagged releases (OSI licence, public pipeline, versions matching tags).
4. **Tests:** against the controller in demo mode.

**Done when:** the client library is published and covers the entity model planned in [Home Assistant](../integrations/home-assistant.md#chosen-approach).
