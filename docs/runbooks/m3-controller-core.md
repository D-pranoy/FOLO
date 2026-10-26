# M3 Controller core

Status: Accepted · Implementation: Partial (RB-06 and RB-07 done)

## RB-06: Node core

**Goal:** the Rust `folo` node: settings, store, identity, supervision, status.
**Depends on:** RB-02, RB-05.

1. **Workspace crates:**
   - `folo-protocol` (generated)
   - `folo-settings`, `folo-store`, `folo-identity`, `folo-supervisor`
   - `folo-node` (binary `folo`)
2. **Settings:** layering per [control plane](../architecture/control-plane.md#settings-layering):
   - managed launches vs foreground launches
   - deployment overrides reported with their source
   - list settings add up
   - `folo reload`
3. **Paths:** the home and data folders of user, service, portable and `--home` launches, the last also for Docker ([folders](../architecture/control-plane.md#folders), [ADR-0010](../decisions/ADR-0010-settings-persistence-and-migration.md), [ADR-0034](../decisions/ADR-0034-the-node-process.md)).
4. **Store:**
   - versioned JSON documents with atomic, debounced writes; revision and server id
   - clamping validation from the generated schemas
   - `secrets.json` with owner-only permissions (POSIX mode 0600; Windows ACL)
   - a document with a structural error stops the node from starting ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D11)
5. **Identity:** X25519 static key and node id; `folo status --json`.
6. **Supervision:**
   - serves the node's port with the loopback source-link endpoint, accepting only the launch token, and binds `127.0.0.1` while remote access is off ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D6)
   - starts `folo-source` with the local link's URL and a launch token in its environment, sends its settings over that link and checks its heartbeat; restarts with backoff ([ADR-0032](../decisions/ADR-0032-local-source-link.md), [supervision](../architecture/control-plane.md#supervision))
   - collects its logs into the node's log
   - `local_source.enabled` and `role` handling
7. **Lifecycle:** signals and service stop shut down cleanly (release outputs, stop workers, flush the store): SIGINT, SIGTERM (which systemd sends) and the Windows console's events in this run book, and the Windows service's stop with the service in [RB-20](m8-release.md#rb-20-windows-service-and-installer). One node per home ([ADR-0034](../decisions/ADR-0034-the-node-process.md) D4).
8. **CLI:** `folo run | status | reload | config import | token show|rotate | pair reset | join <code>` per the [command line](../architecture/control-plane.md#command-line):
   - `status` and `reload` through `GET /system/status` and `POST /system/reload` on loopback, served from this run book on
   - `token` arrives with RB-15, and `pair reset` and `join` with RB-16; until then they say so
9. **First run:** import a legacy `studio.json` found in the home or the working directory (through RB-05, [ADR-0034](../decisions/ADR-0034-the-node-process.md) D10).

**Done when:** the node starts, supervises the engine, persists documents, and reports status. Unit and integration tests are green on Windows and Linux.

## RB-07: Controller API

**Goal:** the client API over one port, conforming to the contracts.
**Depends on:** RB-06.

1. **Server:** axum with tower middleware, on the configured port (default 8777), growing the server RB-06 starts.
2. **Middleware:**
   - Host allowlist
   - client-IP policy: loopback always; private ranges only when remote access is on
   - authentication: loopback clients need no key, and other clients get `401` until client keys arrive with RB-15 ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D1)
   - JSON-only mutating requests; a body limit
   - a strict CSP for UI files; `no-store` for the API
3. **Resources** per [client API](../specs/client-api.md):
   - `info`, `health`, `state`, `zones`, `show`, `effects`, `providers`, `sources`, `library`, `clients`, `updates`, `system`, `auth`
   - endpoints whose subsystems arrive later return `501` until their run book lands ([before their run books](../specs/client-api.md#before-their-run-books))
   - the effect catalog from the contract ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D3)
   - requests to the local source through its link ([requests to sources](../specs/client-api.md#requests-to-sources))
4. **Live WebSocket:** topic subscriptions; binary frames; 30 Hz per client by default; newest data wins under backpressure ([topics and delivery](../specs/client-api.md#topics-and-delivery)). The engine sends the `spectrum` signal and the `levels` notification the topics need ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D6).
5. **Static UI:** embed `ui/dist` (from RB-11) in the node's build ([ADR-0035](../decisions/ADR-0035-serving-the-client-api.md) D7), with hashed assets cacheable and `index.html` uncacheable.
6. **Loopback source link endpoint** for the local source, accepting only its launch token from loopback clients ([the local link](../specs/source-link.md#the-local-link)): RB-06 serves it, and here it moves behind the middleware. Encryption for remote sources arrives in RB-16.
7. **Conformance:** run the RB-02 harness (schemathesis and message fixtures) in CI.

**Done when:** conformance tests pass and the live channel streams engine data.

## RB-08: Zones, projection and adapters

**Goal:** the spatial model and the controller half of the pipeline.
**Depends on:** RB-07.
**Decided in:** [ADR-0036](../decisions/ADR-0036-controller-output.md), with [providers](../architecture/providers.md).

1. **Geometry crate:**
   - zones (bounds, reference object, type), with the origin at the floor's centre
   - target element positions (point, line, grid with wiring orders), and unplaced paths; `display` validated only ([element positions](../architecture/spatial-model.md#element-positions))
   - surface placements (path, plane) and manual mappings
2. **Projector:**
   - sample coordinates per element
   - nearest, bilinear and area-average kernels, at most 64 taps ([sampling kernels](../architecture/spatial-model.md#sampling-kernels))
   - gather plans `(elements, taps)`, cached per configuration revision
   - edge policy; mapping priority and blend; manual ranges with one tap of weight 1 when the counts are equal
   - an abstract `Projector` trait with a CPU implementation
3. **Compositor:**
   - per-surface source priority lists, activity handling, a 300 ms crossfade, optional blend
   - newest frame wins; 500 ms staleness timeout; with no active source, the newest fresh frame of the highest-priority source, else background × master
   - a steppable core, `step(now_us)`, run when a frame arrives and at least every 1/60 s, so tests drive it deterministically
   - **reactive mode and ambient light per zone** ([pipeline](../architecture/pipeline.md#controller-stages)):
     - on / auto / off
     - solid or breathe (30–100% over 8 s)
     - fade in 1.5 s eased, fade out 0.15 s
     - blended into the composited surface before projection, as `colour × level × master`; a fade of 0 leaves the frame byte-exact; the result is truncated
     - silence from the sources' gain-free activity, counted on the controller's clock; the on-pause switch after 0.3 s when at least one linked source has a media session and all of those are paused
   - golden tests against the current engine's ambient frames (within ±1 per channel)
4. **Adapter chain** per [capabilities](../architecture/capabilities.md#adapter-chain-per-target-in-order), in the order resolution → colour model → calibration → latency → flash limiter:
   - resolution (fitting to `address.count`), colour models (RGBW, CCT, dimmer, on/off with hysteresis, xy gamut), calibration, the latency delay line
   - **flash limiter** per zone, off by default ([ADR-0033](../decisions/ADR-0033-flash-limiter-off-by-default.md)), by the [algorithm](../architecture/capabilities.md#algorithm)
   - the rate belongs to step 6's sender
5. **Zone activation and test patterns:**
   - exclusive targets, with conflicts reported for the UI to confirm; `take_over` deactivates the conflicting zones; activation stored in the zone's `active` flag ([client API](../specs/client-api.md#zone-activation))
   - the engine's `config.apply` assigns the surfaces of active zones only
   - `identify`, `sweep` and `position` patterns, refused with `target_busy` for a target another active zone drives ([test patterns](../specs/client-api.md#test-patterns))
   - default surfaces and roles per zone type, applied on creation only ([defaults](../architecture/spatial-model.md#defaults))
6. **Provider interface** ([providers](../architecture/providers.md)):
   - discover, describe (targets with capabilities), connect (the control connection), start (realtime output for a target), send, wire key (what the sender compares), release (per target), status, admin
   - `ElementOutput` now; `RasterOutput` reserved
   - one sender task per provider instance, each at its own rate, sending **only changed frames** plus a per-provider keepalive inside the device's timeout
   - a recording provider for tests
   - in the node: provider states on the live channel (mapped from the client API's), `target:<id>` frames, `POST /providers/discover` and `/providers/{id}/admin/...`
7. **Tests:**
   - geometry unit tests
   - projection golden images (frames stacked into PNGs in `tests/golden/projection/`, written when `FOLO_BLESS=1` is set)
   - ambient inputs from `scripts/make_golden.py --only ambient`, checked against the **existing** `tests/golden/renderer/ambient-*.png` frames; no existing golden is regenerated
   - adapter unit tests, flash limiter tests
   - parity path: identity adapters reproduce the input bytes exactly, on the reactive `renderer/*.png` frames

**Done when:** golden images and adapter tests are green, and parity-path frames equal the engine's output.

## RB-09: Content library

**Goal:** packs, roles and the built-in library as data.
**Depends on:** RB-02, RB-08 (zones and their surfaces, which the documents already hold).
**Decided in:** [ADR-0038](../decisions/ADR-0038-content-library.md), with the [pack format](../specs/pack-format.md), [content](../architecture/content.md) and the [client API](../specs/client-api.md#library). The contract changes (`snap`, `Provenance.hash`, `bindings`, item `description` and `hints`, manifest `categories`, the library's routes and schemas, client API 1.4.0) land with that ADR, the new routes answering `501`; the steps below implement them.

1. **Library service** (a `folo-library` crate, used by the node):
   - built-in packs embedded from `packs/folo/`; installed and local packs in `library/<author>/<name>/`, read and validated at start, failing folders skipped with an error (D1 to D3); the library's limits
   - install with structural validation, limits, compatibility, conflicts and `replace`; clamping into the canonical form, written through a staging folder (D4 to D8)
   - list, manifest, remove (`protected_pack`, `required_by`), deterministic export and the pack hash (D9)
   - item reads and the gallery listing (`/library/items`); item writes and removals in `local/*` packs, with the 60-scene limit of `local/my-presets` (D10); previews served sandboxed
   - `event {kind: library}` on the live channel
2. **Role binding and applying** (D11 to D16):
   - references, the alias table and bare ids (D19)
   - binding by override, role, kind, size and `min_elements`, then fallbacks; skipped roles reported
   - remembered overrides in the show's `bindings`
   - scaling of `cells` regions by `led_count`
   - a show scene only made active; a pack item's copy written in place or as a new scene, with provenance and its item hash; the base in `applied/`, named by the hash of the scene id, removed with its scene
   - `keep_changes`: the three-way merge, and `409 no_base`
3. **Built-ins as data** (D17 to D20):
   - `export_builtins.mjs --trace` and `scripts/build_packs.py generate | check`, writing `packs/folo/core`, `genres` and `effects` and `packs/aliases.json`; the check, and `FOLO_NODE_TESTS=1 uv run pytest tests/test_builtin_packs.py`, in CI's contract job
   - stable ids; descriptions and hints; the categories in `folo/core`
   - equivalence: the built-in golden frames byte for byte at 30, 61 and 144 LEDs; the layers at every length from 1 to 64 and at 100, 144, 255, 256, 300, 512, 1000, 1024, 1999 and 2000; reactions and palettes; `scaleSegments()` scaling
4. **Reset all** on the controller: `POST /show/reset` with the default scene, else `folo/core:default`; master 1 and a black background (D21).
5. **Distribution:** none in v1 (D22). A source answers `packs.install` and `packs.remove` with `-32601` until content that needs them arrives.
6. **Clean-room rewrite gate** (D23, D24):
   - the list in the [effect specifications](../specs/effects/README.md#the-clean-room-rewrite-gate) and `tests/test_clean_room_gate.py`, with `FOLO_RELEASE_GATE=1` in every publishing workflow
   - a behaviour specification for each of the 24 items, then its rewrite from the specification alone in an isolated checkout, with its goldens regenerated; both must be done before any public binary, `edge` images included ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md))
7. **Backup and restore:** `/system/backup` and `/system/restore` (D25 to D27): the four documents, the installed and local packs and the bases; secrets, client keys and `node.toml` excluded; checked as a whole (paths, sizes counted on decompressed bytes, folders matching ids), then replaced through `restore/` and its marker.

**Code changes,** in order; each is one pull request a separate session can implement from the documents:

| # | Change | Steps | Needs |
|---|---|---|---|
| 1 | The engine's `snap: nearest` on `relative` regions, in `surfaces/renderer.py`, with unit tests | 3 (part) | |
| 2 | Built-in packs: the exporter's trace, `build_packs.py`, the committed packs and alias table, the CI check, the equivalence tests, and `tests/test_clean_room_gate.py` | 3, 6 (the test) | 1 |
| 3 | The library service and its routes: load, install, list, manifest, remove, export, items, previews, the gallery, live events; RFC 8785 canonical JSON and item hashes in Python and Rust; the JSON rule's exemption for `application/zip` in the node's middleware | 1 | 2 |
| 4 | Applying and Reset all: references and aliases, binding, bindings, scaling, provenance, bases, `keep_changes`, `/scenes/{ref}/apply`, `/show/reset` | 2, 4 | 3 |
| 5 | Backup and restore | 7 | 3 |
| 6 | The gate step in every publishing workflow, written with `docker.yml` and `release.yml` in [RB-22 and RB-23](m8-release.md) | 6 | 2 |
| 7 | One specification pull request per item, or a few items at a time, by a writer who keeps the clean-room rule | 6 | 2 |
| 8 | One rewrite pull request per item, by an implementer given only its specification | 6 | 7 |

Changes 7 and 8 block every public binary, not the rest of the rebuild.

**Done when:** the API serves every built-in from packs, equivalence tests pass, and the rewrite gate is tracked.

## RB-10: Hyperion provider

**Goal:** Hyperion on the provider interface, with parity and basic administration.
**Depends on:** RB-08 step 6 (the [provider interface](../architecture/providers.md)); step 5 below also needs the rest of RB-08, the controller's output.

1. **Fake first:** a Hyperion JSON server in tests that records frames and answers `serverinfo`, `clear`, `sourceselect` and `componentstate`.
2. **Provider:**
   - JSON client with a reply-draining reader
   - `color` frames with priority, origin and `duration` 2000
   - changed frames only, with a 0.5 s keepalive
   - clear on stop; reconnect with backoff
3. **Describe:** line targets from `serverinfo` LED rectangles on the zone's TV plane; layout watching with proportional mapping scaling.
4. **Administration:** instances, priorities with clear, component toggles, automatic source selection, brightness, token support ([ADR-0014](../decisions/ADR-0014-device-administration-scope.md)).
5. **Parity end to end:** a migrated v1 configuration drives the fake Hyperion with frames byte-identical to the legacy engine for the same synthetic audio.
6. **Live verification** per the [testing policy](../process/testing-policy.md).

**Done when:** the end-to-end parity test is green and live verification has passed.
