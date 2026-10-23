# ADR-0035: Serving the client API

Status: Proposed
Date: 2026-10-23

## Context

RB-07 builds the client API on the node's port ([RB-07](../runbooks/m3-controller-core.md#rb-07-controller-api)). The documents fix a lot of it:
- the resources, conventions and live channel of the [client API](../specs/client-api.md), with their contracts in `specs/`
- the access rules: the Host allowlist, LAN-only client addresses while remote access is on, no key for loopback clients, and a key for every other API and WebSocket call ([ADR-0009](ADR-0009-client-access.md), [security](../architecture/security.md#rules))
- RB-07's steps: the middleware, `501` for endpoints whose subsystems arrive later, a live WebSocket at 30 Hz per client where the newest data wins, the UI from `ui/dist` with hashed assets cacheable and `index.html` uncacheable, a strict CSP, and the conformance harness in CI
- the UI served by the controller and version-locked to it ([ADR-0016](ADR-0016-user-interface.md) D1)
- the signals the node asks its engine for: those a consumer wants, none until RB-07's live channel ([control plane](../architecture/control-plane.md#supervision))

They don't say:
- what a client on another machine gets before client keys exist: they arrive with RB-15, after RB-07
- which endpoints answer `501`, and until which run book; no run book covers backup and restore
- where the effect catalog comes from: each effect declares its id, name, inputs and parameter schema in the effect engine, but a controller in Docker runs no engine
- which live topics are JSON and which are binary frames, what a `status` message holds for each topic, how binary topics get their index, what happens to a topic naming something that doesn't exist, and what "the newest data wins" means for one client
- where the spectrum and the layers' levels come from: the engine publishes only the `beat` and `activity` signals, and no source-link message carries a layer's level
- how the node finds `ui/dist`, which RB-11 builds after the node exists, and what a strict CSP allows
- how requests for a source's inputs, now playing, album art and player controls reach the source, and what a client gets when the source is offline or doesn't answer
- what `/info` reports as the node's name, when no setting names a node, and what `/system` reports before the desktop app (RB-19) and the service (RB-20) exist

## Decision

- **D1.** **Access before client keys.**
  - Every request goes through these checks in order: the Host allowlist, then the client's address (loopback always; the private ranges of [security](../architecture/security.md#rules) only while remote access is on), then authentication, then the JSON rule and the body limit for requests that reach a route.
  - Loopback requests need no key. Every other request to `/api/` needs a client key or a session, except `/api/v1/health` and the endpoints that use their own proofs: the source link, `/auth/login` and `/auth/requests`. Until client keys and sessions arrive with RB-15, those other requests get `401`, WebSocket upgrades included.
  - The UI's files need no key.
  - The source link takes the `local` purpose from loopback clients only. Its other purposes are rejected until RB-16.
- **D2.** **Later subsystems.** An endpoint whose subsystem arrives with a later run book answers `501` with the error code `not_implemented` and that run book in `details.runbook`, until it lands. The [client API](../specs/client-api.md#before-their-run-books) lists them.
  - **Backup and restore** arrive with RB-09, because a backup holds the library's packs.
  - **`/system`** reports the remote access settings the node runs with. Start at sign-in reads `false` until the desktop app (RB-19). The service's `installed` and `running` both say whether this node was launched with `--service`, until the service (RB-20). Changing `/system` arrives with RB-15.
- **D3.** **The effect catalog is contract data.** The schema of the show document lists every built-in effect's id, version, name, description, inputs and minimum element count, next to the parameter schemas it already holds. The node serves `/effects` from the contract, so a controller without an engine serves it too. Each parameter schema is served with its references resolved, so it stands alone. The engine's registry declares the same effects, which a test checks. A new effect goes into the contract first.
- **D4.** **Live topics.** `spectrum:<source>` (signal frames), `surface:<id>` and `target:<id>` (surface frames) are binary topics. The others are JSON:
  - A `status` message holds what its client subscribed to. `status` gives every source's state, every zone and every provider. `zone:<id>` gives that zone. `input:<source>`, `beat:<source>` and `media:<source>` add that part of the source's entry. A client subscribed to none of these gets no `status` messages.
  - `levels:<source>` gives `levels` messages, one per surface of the source, each naming its surface.
  - `config_changed` and `event` messages go to every client, subscribed or not.
  - A topic may name something that doesn't exist yet. It is acknowledged, and its data flows once there is some.
- **D5.** **Live delivery.** Each client gets at most 30 updates a second. Each update carries the newest value of everything that changed since the last one, and nothing queues up:
  - one `status` message; one `levels` message per surface; one frame per binary topic
  - `config_changed` carries each document's newest revision, and `event` each job's newest state
  - A frame goes to the client as its source sent it, with the index its topic has on that connection and without LZ4.
  - `subscribed` answers each `subscribe` with the topics it added, and each binary topic's index: the lowest the connection isn't using. A connection holds at most 64 topics; those past the limit are left out of `subscribed`.
  - A client message that isn't a `subscribe`, `unsubscribe` or `auth` message closes the connection with status 1008.
- **D6.** **Spectrum and levels from sources.**
  - **The `spectrum` signal:** 192 bands, spaced logarithmically from 20 Hz to 20 kHz. Each band's level is the loudest bin in its range, interpolated when the band is narrower than a bin, with the input's gain, in dB and at least −120: the current studio's display bands.
  - **The `levels` notification:** `{surface, layers: {id: [level dB, value 0–1, average dB]}}`, for each surface `config.apply` lists in `publish.levels`.
  - The engine sends both with its rendered steps, at most 60 times a second each.
  - The node asks for the spectrum, the beat and the levels only while a live client subscribes to them.
- **D7.** **The UI is built into the node.** A node build embeds the files of `ui/dist`, so each node serves the UI it was built with. A build without them serves a page that says so.
  - Files under `assets/`, which the UI's build names by their content hash, are cacheable for a year and immutable. Every other file, `index.html` included, is `no-store`.
  - The UI's files carry a Content Security Policy that allows scripts, connections, fonts and images from the node only (images also from `data:` and `blob:` URLs). It forbids plugins, framing and other base URLs. Styles may also be inline, because Svelte's transitions write inline styles. The policy's text is in [security](../architecture/security.md#rules).
  - API responses are `no-store`.
- **D8.** **Requests to sources.** The node answers a source's `inputs`, `media/art` and `media/control` through that source's link, with `inputs.list`, `media.art` and `media.control`. It answers `media` from the source's latest `media` notification.
  - A source that isn't linked gets `503`, and one that doesn't answer within 5 s gets `504`.
  - The source `local` is the node's own engine while the node runs it, even when `sources.json` doesn't list it.
- **D9.** **The node's name** in `/info` is the computer's host name.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Serving other machines without keys until RB-15 | The UI could be tried from a phone sooner | Against the security rules ([ADR-0009](ADR-0009-client-access.md) D2): anyone on the LAN could drive the lights |
| `501` for the API's documents until their subsystems exist | Less to build | Zones, the show and the sources are documents the node already keeps, and the engine already renders them |
| The engine reports its effects over the source link | One declaration, in the engine | A controller without an engine has no catalog; sources of different versions disagree |
| A fixed `status` message, whatever the topics | Simpler | Clients get data they didn't ask for; the per-source topics would mean nothing |
| Queueing live messages per client | Nothing is skipped | A slow client builds a backlog of stale data without bound |
| Layer levels as signal frames, in layer order | No new message | The node would have to know the engine's layer order; the ids are lost |
| UI files from a folder beside the executable | Replaceable without a rebuild | A second artifact to keep in step with the node, and a setting to find it in a development checkout |
| A Content Security Policy without inline styles | Stricter | Svelte's transitions write inline styles |
| Backup and restore in RB-07 | Done sooner | A backup without the library's packs, which RB-09 brings |

## Consequences

- **Contracts:** additive changes only.
  - The [source link](../specs/source-link.md) gains the `levels` notification and `publish.levels`: version 1.3.
  - The live channel's `levels` message gains its surface: client API version 1.2.0.
  - The show schema gains the catalog's fields (`x-folo-effects`).
- **Documents:**
  - the [client API](../specs/client-api.md): the request checks, the endpoints before their run books, the live topics and delivery, and requests to sources
  - the [source link](../specs/source-link.md) and the [pipeline](../architecture/pipeline.md#publishing): the spectrum signal and the levels notification
  - [security](../architecture/security.md#rules): access before client keys, and the Content Security Policy
  - the [control plane](../architecture/control-plane.md#supervision): the signals and levels the node asks for
  - [M3](../runbooks/m3-controller-core.md): RB-07 steps 3, 4 and 5, and RB-09's backup and restore
- **RB-07** includes the engine's side of D6. The conformance harness runs against a node on loopback, which needs no key, so checking authentication waits for RB-15. It counts the documented `501`, `503` and `412` answers as conforming, and expects document bodies to be clamped rather than rejected.
- **Build order:** release builds build the UI before the node (RB-11, RB-23).
