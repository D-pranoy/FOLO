# ADR-0004: Implementation languages and contracts

Status: Accepted
Date: 2026-10-13

## Context

**Measured with the current Python engine:**
- 0.4–2.2 ms per 10 ms audio packet on realistic setups, and up to 13 ms at the schema maximum.
- Controller-side projection in numpy: about 0.8 ms for 5000 elements from 8 sources.
- End-to-end latency is dominated by the analysis windows (43 and 85 ms), device cadences (WLED up to 15 ms, Hue about 40 ms) and smoothing.

**What the controller has to do:** networking, cryptography, device protocols (DTLS, UDP, HTTPS), service hosting, updates, and later GPU projection. The desktop shell (Tauri) is Rust.

**The Rust crate check** (October 2026) found mature crates for all of this:
- axum, utoipa, snow (Noise), the openssl crate (DTLS-PSK), rustls, mdns-sd, windows-service
- minisign-verify, self-replace, windows-capture, ashpd, pipewire, wgpu, wasmtime

## Decision

- **D1.** The **node** (controller and source roles, supervision, networking, providers, projection, updates) is written in **Rust**.
- **D2.** The **effect engine** (`folo-source`: audio inputs, analysis, effects, surfaces) stays in **Python 3.13**, restructured from the current engine. Hot paths may move to Rust through PyO3/maturin when profiling shows a need.
- **D3.** The **display capture agent** (`folo-capture`) is written in **Rust**.
- **D4.** The **UI** is Svelte 5 with TypeScript (strict) and Vite. The **desktop app** is Tauri v2 (Rust), pinned to 2.x for version 1.
- **D5.** **Contracts first:**
  - JSON Schema, OpenAPI 3.1 and AsyncAPI 3.0 in `specs/` are the single source for models and messages.
  - Types are generated: pydantic for Python, serde via typify for Rust, openapi-typescript for TypeScript.
  - Conformance tests run against both the Rust and Python implementations.
- **D6.** Go is not used: it would be a fourth language with no code sharing.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Python controller | Fastest start; shares code with the engine | Large Docker image; DTLS-PSK on 3.13 depends on a young library; GPU projection would mean a later rewrite |
| Go controller | Simple language; excellent DTLS (pion) | Fourth language; shares nothing with Tauri; weak GPU path |
| Rewrite the engine in Rust too | One backend language | Throws away tuned, working DSP for no measurable gain |

## Consequences

- Toolchains: uv, cargo and npm. CI builds and tests all three.
- Shared types must be generated, never hand-copied.
- The effect engine needs no networking or cryptography beyond loopback.
