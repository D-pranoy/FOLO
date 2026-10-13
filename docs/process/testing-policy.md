# Testing policy

Status: Accepted

Decided in [ADR-0024](../decisions/ADR-0024-testing-strategy-and-device-safety.md).

## Test layers

| Layer | What | Where |
|---|---|---|
| Unit | Pure logic in every language | Every PR |
| Golden | Renderer frames from synthetic audio (seeded); migration outputs; projection images; packet bytes (HueStream, DDP, DNRGB); black-bar detection | Every PR |
| Contract | OpenAPI and AsyncAPI conformance against the Rust and Python implementations; Python and Rust clamping agree on a shared corpus | Every PR |
| Integration | The node with fakes: Hyperion JSON server, WLED UDP receiver, Hue bridge double (REST plus `openssl s_server` DTLS-PSK), multi-node source links | Every PR |
| End to end | Playwright against a node in demo mode (synthetic audio, fake providers); Chromium and WebKit; phone, tablet and desktop sizes with touch emulation | Every PR, once the UI exists |
| Packaging | Install, upgrade and uninstall matrices (Windows NSIS; deb, rpm and Arch in containers); update dry-runs | Release builds and nightly |
| Live verification | Real devices and real sessions, by maintainers | Before each milestone closes and before releases |

## Demo mode

`folo run --demo` starts a controller with:
- a synthetic audio input (deterministic music-like signal) and a synthetic screen input (replayed frames)
- fake providers
- a seeded configuration

Demo mode is used for end-to-end tests, documentation screenshots and UI development.

## Live-device safety rules

1. **Never** test against someone's live configuration or devices without their consent. Use a separate data folder (`--data-dir`) and a separate port for experiments.
2. **Announce** what will change on a device or bridge (pairing, entertainment configurations, segments, settings) before doing it.
3. Start at **low brightness** with **one target at a time**. Keep the flash limiter on.
4. **Release** devices afterwards: stop streams, clear priorities, restore settings you changed. Confirm the user's configuration is untouched.
5. Hue: stop Hue Sync (or any other streaming app) before a streaming test. Never take over another app's stream without the device owner's confirmation.
6. Bridges and controllers can only be changed while their owner can observe the result.

## Golden files

- Regenerate goldens only when behaviour is meant to change, and say why in the PR.
- Review image goldens (stacked frames rendered as PNGs) visually.
- Synthetic inputs and recorded fixtures are committed. Recordings of real audio or screens are never committed.
