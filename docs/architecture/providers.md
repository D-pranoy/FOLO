# Providers

Status: Accepted · Implementation: Not started

A provider is the controller's output backend for one kind of device: Hyperion, WLED or Philips Hue ([glossary](../glossary.md)). This page is the interface every provider implements, its lifecycle, the sender that drives it, and how discovery, description and administration work. The stages before it are in [pipeline](pipeline.md#controller-stages) and [capabilities](capabilities.md), decided in [ADR-0036](../decisions/ADR-0036-controller-output.md). What each device speaks is in its integration document: [Hyperion](../integrations/hyperion.md), [WLED](../integrations/wled.md) and [Hue](../integrations/hue.md).

## Kinds and instances

- A **kind** (`hyperion`, `wled` or `hue`) is code: a `ProviderKind` that knows how to discover devices and make instances. The node holds a registry of the kinds it has.
- An **instance** is a configured provider in `providers.json`: its id, name, `enabled`, `rate_hz` and the settings of its kind ([configuration model](../specs/config-model.md#entities-summary)). A target's `provider.instance` names one.
- An instance's `output` says which interface it implements: `element` (the default) or `raster`. Only `element` is built. `RasterOutput` is reserved for devices that take whole images, and is validated but never executed in v1, as the `display` target kind is ([projection-mapping groundwork](projection-mapping-groundwork.md)).
- A kind without an implementation yet has no entry in the registry. Its instances exist as configuration, report `off` with the message "arrives with RB-NN", name the run book that brings it, and drive nothing.

## The interface

Every instance offers these operations. All of them are asynchronous, and none of them may block the sender of another instance.

| Operation | What it does |
|---|---|
| **discover** | Finds devices of the kind on the network (a kind-level operation, no instance needed). Returns `DiscoveredDevice`s: kind, id, name, address. |
| **describe** | Asks the connected device what it has: its targets with their address ranges, element counts, [capability descriptors](capabilities.md#capability-descriptor) and, where the device knows them, layouts. |
| **connect** | Opens the **control connection**: reaches the device, authenticates, and leaves it ready for describe and administration. It starts no realtime output. Called again after a loss. |
| **start** | Starts realtime output for a target: Hue's entertainment stream, WLED's realtime mode. Hyperion needs nothing here. Called when an active zone, or a test pattern, first drives the target. |
| **send** | Takes the element colours of an instance's targets for one frame. |
| **wire key** | Gives the bytes (or a digest of them) the provider would send for a target's frame: the frame as the device receives it, after the provider's truncation and encoding. The sender compares keys to find changed frames. |
| **release** | Gives one target's lights back: clears the priority (Hyperion), ends realtime (WLED), closes the stream (Hue). It takes the target; releasing all of an instance's targets is the instance's release. |
| **status** | The instance's state, and a message when there is something to say. The instance's sender keeps it, from how the other calls end; a provider doesn't report it. |
| **admin** | The basic administration of [ADR-0014](../decisions/ADR-0014-device-administration-scope.md), by action name. |

- **`ElementOutput`** is the interface of `send`. A frame holds, per target, one colour per element **in the target's colour model**: on/off, dimmer, CCT, RGB, RGBW, RGB+CCT or xy ([capabilities](capabilities.md)). RGB values are on the effect engine's 0–255 scale and the provider truncates them to the bytes its device takes, which is what keeps [parity](pipeline.md#parity-mode) byte-exact.
- **`RasterOutput`** is reserved and has no operations yet.
- **Delivery.** Each target's effective capabilities (the discovered descriptor with the user's overrides on top) say whether it is `streaming` or `state`, and the instance's sender follows that ([the sender](#the-sender)).
- A provider is the only code that knows its device's protocol. Adapters never branch on the kind, and providers never convert colours: the colour model was settled by the adapter chain.
- Tests use a recording provider that implements the interface and keeps every frame it is sent.

## Lifecycle

1. **Created** when `providers.json` lists an enabled instance whose kind is implemented. A disabled instance, or one of a kind without an implementation, stays `off`.
2. **Connecting.** `connect` runs at once and whenever the control connection is lost, so describe and administration work whether or not a zone is active. A failure is retried with backoff, doubling from 1 s up to 60 s, each delay cut by up to 25% at random, as the node does for its engine ([control plane](control-plane.md#supervision)).
3. **Describing.** After each `connect`, `describe` runs, and [what it writes](#describe-discover-and-administration) is applied.
4. **Starting and running.** The first time a target is driven by an effectively active zone, or by a running [test pattern](../specs/client-api.md#test-patterns), `start` runs for it, and the sender then sends its frames. An instance that drives nothing therefore opens no realtime output: Hue's bridge keeps the entertainment area for its own app, and its stream, which closes after 10 s without data, is never left to time out. A send that fails ends the connection, and the instance goes back to connecting; running targets start again when it is back.
5. **Releasing.**
   - **A target** is released when nothing drives it any more: its zone was deactivated or changed, or its test pattern ended. A target another active zone drives stays on.
   - **The instance** is released when it is disabled or removed, and when the node stops: it releases each of its targets and closes the connection. A node that stops releases its outputs, then writes its pending documents ([control plane](control-plane.md#supervision)).
   - A release that fails is logged, and the device's own timeout ends the output ([the sender](#the-sender)).

### States

The client API and the live channel each have a list of states, which the node maps as follows, without a contract change. **`connected` means the control connection is up**, whether or not any target is driven; what the lights show is on the `target:` topics.

| Situation | Client API `ProviderStatus.state` | Live `ProviderLive.state` |
|---|---|---|
| Disabled, or a kind without an implementation; nothing runs | `stopped` | `off` |
| Running, with no connection and nothing to report: starting, or between a clean close and the next attempt | `connecting` | `disconnected` |
| The control connection is up | `connected` | `connected` |
| The last attempt, start or send failed; the message says why, and attempts continue with backoff | `error` | `error` |

`ProviderStatus` has no message. The live channel's `message` is empty except in `off` for a kind without an implementation ("arrives with RB-NN") and in `error`.

## The sender

One sender task runs per enabled instance, at the instance's `rate_hz` (1–100; 40 unless the kind's run book sets another default for new instances, as the overview's 60 Hz for WLED and 50 Hz for Hue are), and never faster than the lowest `limits.max_rate_hz` of its targets. It is the stage after the [adapter chain](capabilities.md#adapter-chain-per-target-in-order).

- **Newest wins.** The sender keeps the newest frame per target. Nothing queues up: a sender that falls behind sends the newest frame and skips the rest.
- **Streaming targets:** at each tick, if the frame **changed** since the last one sent (its **wire key** differs from the last one sent), it is sent. An unchanged frame goes out again only as a **keepalive**, inside the device's timeout:

  | Provider | Keepalive, at the latest | Device timeout |
  |---|---|---|
  | Hyperion | 0.5 s | `duration` 2000 ms, after which Hyperion releases the LEDs |
  | WLED | 1 s | realtime timeout, 2.5 s by default |
  | Hue | 5 s | the stream closes after 10 s without data |

- **State targets:** updates are paced by a token bucket per bridge or device, sent only on a visible change, with a device-side transition equal to the update interval ([capabilities](capabilities.md#adapter-chain-per-target-in-order)).
- **Safety timeouts** release the lights if FOLO stops sending: `duration` for Hyperion, the realtime timeout for WLED and the closed stream for Hue.
- **No frame yet.** Until the first frame of a target arrives, the sender sends nothing for it.
- **One instance's trouble stays its own:** a slow or failing device delays only its own sender.

## Describe, discover and administration

- **Discovery** (`POST /providers/discover`) asks every registered kind in the request's `kinds` (all, by default) and gathers the answers into one list. Kinds without an implementation add nothing, so the list is empty until RB-10 brings the first. Discovery changes no configuration.
- **What describe writes.** Only a target's `capabilities.discovered`, and only when it differs from what is stored. Everything else describe learns, such as the layouts a device offers (Hyperion's LED rectangles on the TV plane, WLED's segments), is offered to the zone editor and never written by itself. The exception is RB-10's layout watching, which rescales mappings proportionally when Hyperion's LED count changes ([Hyperion](../integrations/hyperion.md#folo-provider-design)). A user's overrides are never touched.
- **Administration** (`/providers/{id}/admin/...`, `admin` scope):
  - `GET` reads: the device's current values and, for an action, **what a `POST` would change** ([ADR-0014](../decisions/ADR-0014-device-administration-scope.md) D3).
  - `POST` makes the change, and answers with the new values.
  - An instance of a kind without an implementation gets `501 not_implemented` naming its run book in `details.runbook`; an action the kind doesn't have gets `404 not_found`. A device that refuses or doesn't answer gets `502 provider_error`.
  - What each kind offers is listed under its integration's basic administration.
