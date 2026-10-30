# Hyperion.NG

Status: Accepted · Implementation: Partial (the provider, its describe and its administration, tested against a fake Hyperion; the end-to-end parity test and live verification are pending, [RB-10](../runbooks/m3-controller-core.md#rb-10-hyperion-provider))

Decided in [ADR-0011](../decisions/ADR-0011-hyperion-provider.md).

## Protocol

- **JSON API** over TCP (default port 19444). Commands are newline-terminated JSON. Hyperion answers every command, so replies must be read continuously, or the socket buffers fill up.
- **Frames:** `{"command":"color","color":[r,g,b,…],"priority":100,"origin":"FOLO","duration":2000}`
  - `origin` must be 20 characters or fewer.
  - `duration` makes Hyperion release the LEDs if FOLO stops sending.
- **Layout:** `{"command":"serverinfo"}` → `info.leds[]` with `hmin/hmax/vmin/vmax`, a rectangle per LED on the screen.
- **Useful admin commands** (no authentication on default installs):
  - `{"command":"clear","priority":N}`
  - `{"command":"sourceselect","auto":true}`
  - `{"command":"componentstate","componentstate":{"component":"<C>","state":<bool>}}`
  - Other commands may need an API token when Hyperion's authentication is on.
- A FlatBuffers interface exists (port 19400). It is a later efficiency option.

## FOLO provider design

- **Targets:**
  - One line target per Hyperion instance, with the LED count from `serverinfo`.
  - Element positions come from the LED rectangles, placed on the zone's TV plane: x from the rectangle's horizontal centre, z from its vertical centre. Hyperion setups therefore place themselves.
- **Layout changes:** watched (polling `serverinfo`). Mappings scale proportionally, as the current studio does.
- **Output:** one sender per instance at its configured rate (default 40 Hz). Priority and origin are configurable. On stop, FOLO clears its priority.
- **Parity:** migrated studio setups run through this provider unchanged ([pipeline](../architecture/pipeline.md#parity-mode)).

### How it is built

The crate `folo-provider-hyperion` implements the [provider interface](../architecture/providers.md) (RB-10 steps 1 to 5). The kind is `HyperionKind`, which the node registers.

- **Connections:** a Hyperion server runs instances, and a JSON connection drives the one it has selected (`instance` / `switchTo`). The provider keeps **one connection per running instance**, each switched to its instance, because the sender drives the targets side by side. `connect` opens the first, and `describe` (or an administration request) opens the others and opens again one that died; a target whose connection isn't open yet is refused by `start`, and the sender tries it again. The connection's reader reads every reply and drops those nobody asked for.
- **Targets:** one line target per running instance. Its id is the instance number (`"0"`, `"1"`), its name Hyperion's friendly name, and its address `start` 0 and `count` the number of LEDs in `serverinfo`. It is RGB, pixel (a single LED is `single`) and streaming. An instance that stops running loses its target, and a target with no LEDs, or more than 65535, isn't offered.
- **Layout:** each target offers one layout, a line geometry with the LED rectangles' positions (up to 2000; a bigger setup offers none). **describe doesn't know the zone,** so the layout is on the default TV plane (1.2 × 0.68 m centred on the origin, facing the room, [spatial model](../architecture/spatial-model.md#defaults)): x from the rectangle's horizontal centre, z from its vertical centre, y 0. `place_on_plane` places the same rectangles on the TV plane of the zone being edited, which the controller calls. describe changes nothing on Hyperion: it logs in and switches the connection's instance, which is state of the connection only.
- **Target ids and the migration:** a target whose id is a Hyperion instance number drives that instance. Any other target, such as the migration's `strip`, drives the default instance 0, which the legacy studio drove: a target's address (`start`, `count`, `channels`) has no field for a Hyperion instance number, so there is nothing else to resolve it from. The parity test runs the migrated documents as they are.
- **Token:** a token set with `POST token` is kept by the node in `secrets.json` (`providers.<id>.token`, through `Provider::secrets`) and read when the instance is made again. Hyperion with authentication on refuses the connection without a token, and the action needs a connection, so the first token has to be written to `secrets.json` or entered while authentication is off.
- **Layout watching:** the proportional scaling of mappings when Hyperion's LED count changes (`layout::scale_span`) is not wired to the store.
- **Frames:** one `color` command per target and frame, with the instance's `priority`, its `origin` (4 to 20 characters) and `duration` 2000, written as the legacy sender writes it (Python's `json.dumps` spacing and escapes; the golden `tests/golden/hyperion/legacy.json` is captured from the legacy client). Levels are truncated to bytes, a frame with no LEDs sends nothing, and the wire key is those bytes, so a change below one level isn't sent. The delivery policy has the 0.5 s keepalive. The legacy sender's origin was `Audio Effects`, a new instance's is `FOLO` (the default in the [configuration model](../specs/config-model.md)), so a byte comparison with the legacy engine sets the origin to the legacy one.
- **Release:** a target's release is `clear` for the instance's priority on its connection (harmless for a target that isn't running); the instance's release closes the connections after the sender released each target.
- **Authentication:** with a token in the instance's secrets, each connection logs in (`authorize` / `login`) before anything else. Without one, a Hyperion that needs a token refuses the first command, and the instance reports `error` with Hyperion's message.
- **Layout watching:** the sender describes again every `DESCRIBE_EVERY` (5 s, as the legacy layout watcher polls) if the node sets `describe_every`. `layout_changes` finds the targets whose LED count differs between two describes. Mappings scale **as the studio scales segments** (`scale_segments`: start and end times the ratio, rounded half to even, the end never before the start) in `scale_span`, `scale_address` and `scale_ranges`; the golden checks them against the studio's own answers. Applying them to the stored documents is the controller's, with the store.
- **Discovery** finds nothing: setups are entered by address.

### Administration actions

The actions of `/providers/{id}/admin/{action}` ([providers](../architecture/providers.md#describe-discover-and-administration)). Each answers its current values; a `GET` adds `change`, a sentence on what a `POST` would change, naming the values when the `GET` carries them as query parameters ([ADR-0014](../decisions/ADR-0014-device-administration-scope.md) D3). Except `instances`, `token` and `ui`, an action takes `instance` and otherwise applies to the selected instance: the lowest running one at first, then the one `instances` selected, which a reconnect keeps while it runs.

| Action | Values | `POST` body |
|---|---|---|
| `instances` | `instances[] {instance, name, running}`, `selected` | `instance`: the instance administration applies to |
| `priorities` | `priorities[] {priority, origin, owner, component, active, visible}`, `auto` | `priority`: 1–253 to clear it, or -1 for all |
| `components` | `components[] {name, enabled}`, `switchable` | `component` (`LEDDEVICE`, `SMOOTHING`, `BLACKBORDER`, `AUDIO` or `V4L`) and `enabled` |
| `source` | `auto`, `visible` (the shown priority) | `auto: true`, or `priority` to pin one |
| `brightness` | `brightness` (0–100) | `brightness`, set on every colour adjustment of the instance |
| `token` | `required` (Hyperion asks for one), `configured`, `persisted` (always false) | `token` (in the body only: a `GET` ignores it, because it would travel in a URL): logs in on a connection of its own, and the provider uses it from then on |
| `ui` | `url`: Hyperion's own UI, on port 8090 | none |

- **Tokens** are kept in the node's `secrets.json`, which the provider doesn't write: an accepted token is used in memory only, and `persisted` says so. A token is never echoed or printed, and one under 36 characters is refused before it is sent, because Hyperion doesn't answer it. Creating tokens stays in Hyperion's UI.
- **Bad requests** (a priority out of range, a component that isn't switchable) are answered as a refusal, which is `502 provider_error`, because the provider interface has no bad-request error.
- **LED layout view and import** are the target's layouts from describe. Device configuration and effect scripting stay in Hyperion.

### Sources

The requests and replies were checked against Hyperion.NG's source (`hyperion-project/hyperion.ng`, the default branch at commit `9a73e822637ed9e6c5c1fa1bc84e9adc527dc604`, which is later than release 2.2.1), read only:

- **Requests,** `libsrc/api/JSONRPC_schema/`: `schema-color.json` (`priority` 1–253, `origin` 4–20 characters, `duration`, at least 3 levels), `schema-clear.json` (`priority` -1…253, required), `schema-sourceselect.json` (`priority` or `auto`), `schema-componentstate.json` (the component names), `schema-adjustment.json` (`brightness` 0–100, `id`), `schema-instance.json` (`switchTo`) and `schema-authorize.json` (`login`, `tokenRequired`, `token` at least 36 characters). Every command may carry `tan`, and has no other unknown field.
- **Replies,** `libsrc/api/JsonAPI.cpp` and `JsonInfo.cpp`: a reply has `success`, `command` (the command, with `-subcommand` for `authorize` and `instance`, as in `authorize-login`) and `tan`; data is under `info`, an error under `error` (`No Authorization` when the connection isn't logged in). `serverinfo`'s `info` has `leds` (the instance's LED setting: `hmin`, `hmax`, `vmin`, `vmax` as fractions), `instance[] {instance, friendly_name, running}`, `priorities[] {priority, owner, componentId, origin, active, visible, value}`, `priorities_autoselect`, `components[] {name, enabled}` and `adjustment[] {id, brightness, …}`. `instance`/`switchTo` answers `info {instance}`; `authorize`/`tokenRequired` answers `info {required}`.
- **Behaviour worth knowing:** `sourceselect` answers success whatever the priority (the administration checks the priority is in the list first). A `login` token shorter than 36 characters gets no answer at all, so the provider refuses one itself. `clear` accepts -1 and 1–253. For a local connection a token is required only when Hyperion's local authentication is on; any other connection always needs one.

What wasn't checked against a running Hyperion: the live exchange itself (RB-10 step 6).

### Tests

`folo_provider_hyperion::testing::FakeHyperion` (the `testing` feature) is the Hyperion JSON server of the [testing policy](../process/testing-policy.md): newline-delimited JSON on loopback, recording every command and answering `serverinfo`, `color`, `clear`, `sourceselect`, `componentstate`, `adjustment`, `instance` and `authorize`. It can require a token, drop its connections and refuse new ones. Nothing in these tests touches a real Hyperion.

## Basic administration

- instance list and selection
- LED layout view and import
- **priority list, with clear**
- component toggles: LED device, smoothing, black border, audio, V4L
- automatic source selection
- basic brightness
- API token support
- open Hyperion's own UI

Device configuration and effect scripting stay in Hyperion.
