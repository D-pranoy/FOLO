# Configuration model

Status: Draft · Implementation: Partial (JSON Schemas, generated types and clampers; nothing reads the documents yet)

The controller's persistent model. Validation **clamps values to their limits and never rejects** a document, except for structural errors. Hand-written or stale input therefore always results in a working configuration, matching today's `sanitize_config()` behaviour.

**Machine-readable form:** JSON Schemas in `specs/schema/` (draft 2020-12), one file per document plus `common.json`. Limits are `minimum`/`maximum`, `maxLength` and `maxItems`; every field has a `default`. The [clamping rules](#clamping-rules) say how the clampers apply them. This page and the schemas must agree; a test compares the tables here with the schemas.

## Documents

Each document has `schema_version` (2), `revision` and `server_id`. Writes are atomic (temporary file, fsync, replace) and debounced.

| Document | Contents | Schema |
|---|---|---|
| `node.toml` | Host settings: role, listen address and port, data paths, remote access, log level. Overridable by `FOLO_*` and CLI flags ([control plane](../architecture/control-plane.md#settings-layering)). | `node.json` |
| `zones.json` | Zones, targets, surfaces, mappings | `zones.json` |
| `show.json` | The active scene per zone, the scenes with their layers, and the default scene. A surface's master and background are surface fields in `zones.json`. | `show.json` |
| `providers.json` | Provider instances (Hyperion, WLED, Hue) and their settings | `providers.json` |
| `sources.json` | Paired sources, their owner or consumer role, assigned surfaces, inputs and their settings | `sources.json` |
| `clients.json` | Client keys (stored hashed), names, scopes, last use | `clients.json` |
| `library/` | Installed packs and the local "My presets" pack ([pack format](pack-format.md)) | `pack.json` |
| `secrets.json` | Provider credentials, identity keys. Owner-only permissions; never exported or returned. | `secrets.json` |

## Entities (summary)

**Ids** are strings of 1–64 characters. A missing, empty or duplicate id gets a new one: 8 lowercase hexadecimal characters, as today. **Names** are up to 64 characters (scene and layer names up to 40, as today).

**Zone:** `id`, `name`, `type` (`music | screen | desk | room | custom`, default `custom`), `bounds {x, y, z}` in metres (0.1–1000, default 4 × 4 × 2.5), `reference {kind, position, orientation}`, `targets[]`, `surfaces[]`, `mappings[]`, `flash_limiter {enabled, exempt_reason?}`, `hue_sync {mode, configuration_id?}`, plus:

| Field | Range / values | Default |
|---|---|---|
| `reactive` | `on`, `auto`, `off` | `on` |
| `ambient.color` | `#RRGGBB` | `#4B00FF` |
| `ambient.style` | `solid`, `breathe` | `solid` |
| `ambient.level` | 0–1 (linear output scale) | 0.35 |
| `ambient.idle_after` | 1–600 s | 5 |
| `ambient.on_pause` | boolean: in `auto`, switch after 0.3 s of silence when the player is paused | true |
| `beat_offset_ms` | −200…200 (beats fire earlier when positive) | 0 |
| `album_colors` | boolean: layers take colours from the active source's album art | false |

- `reference.kind`: `tv`, `listening`, `desk` or `none` (default); `position {x, y, z}` in metres (−1000…1000); `orientation {yaw, pitch, roll}` in degrees (−180…180).
- `flash_limiter.enabled` defaults to true; `exempt_reason` (up to 200 characters) records why a zone opted out.
- `hue_sync.mode`: `off` (default), `managed` or `linked`.

**Target:**
- `id`, `name`
- `kind`: `point | line | grid | display`, where display is reserved and never executed
- `provider {instance, address}`: the provider instance's id, and the address range on it: `start` (0–65535), `count` (1–65535) and, for Hue, `channels` (channel ids 0–255, up to 20)
- `geometry`, by kind. It carries the same `kind`; geometry of another kind is replaced by the default for the target's kind.
  - point: `position`
  - line: `path` (2–64 points) + `count` (1–2000), or explicit `positions` (up to 2000)
  - grid: `origin`, `u`, `v`, `width` and `height` (1–256 each), `wiring {order: row | column, serpentine, start: top_left | top_right | bottom_left | bottom_right}`
  - display: `rect {origin, u, v}`, `resolution {width, height}` (1–16384), `warp {kind: none | homography | mesh, matrix?, mesh?}`, `mask?`
- `capabilities {discovered, overrides}`, each a descriptor ([capabilities](../architecture/capabilities.md)) whose fields are all optional in `overrides`:
  - `color_model`: `onoff`, `dimmer`, `cct`, `rgb` (default), `rgbw`, `rgbcct`, `xy`
  - `addressing`: `single`, `zoned`, `pixel` (default)
  - `delivery`: `streaming` (default) or `state`
  - `limits {max_rate_hz (0.1–1000), latency_ms (0–5000), cct_min_k, cct_max_k (1000–20000), gamut {red, green, blue} as CIE xy, min_brightness (0–1)}`
- `calibration`:

| Field | Range / values | Default |
|---|---|---|
| `gamma` | 0.1–5 | 1 |
| `white_balance {r, g, b}` | 0–1 each | 1, 1, 1 |
| `brightness_min` | 0–1 | 0 |
| `brightness_max` | 0–1 | 1 |
| `color_order` | `rgb`, `rbg`, `grb`, `gbr`, `brg`, `bgr` | `rgb` |
| `latency_ms` | 0–5000 | 0 |

**Surface:** `id`, `kind` (`strip | canvas`), `resolution {width, height}` (strip: 1–2000 × 1; canvas: up to 1024 × 1024, published at most 128 × 72), `placement` (`none`, a `path` of 2–64 points, or a `plane {origin, u, v}`), `role` (for example `strip.main`; up to 64 characters), `background` (`#RRGGBB`, default `#000000`), `master` (0–1, default 1), `sources` (source ids in priority order) and `blend_sources` (blend every active source instead of picking one; default false).

**Mapping:** `id`, `surface`, `target` (ids), `method` (`plane | path | manual`, default `manual`), `kernel {type: nearest | bilinear | area, radius (0–1 of a cell's neighbourhood, default 0.5)}`, `edge` (`clamp` (default) or `none`), `priority` (0–100, default 0), `blend` (`max` (default) or `alpha`), plus manual `ranges[]`: `{cells: {start, count}, elements: {start, count}, reversed}`.

**Scene:** `id`, `name`, `tags` (up to 16 of up to 24 characters), `provenance? {pack, version, item}`, `layers_by_role {role: Layer[]}` (up to 64 layers per role), `macros[]` (up to 8: `id`, `name`, `value` 0–1 (default 0.5), `targets[] {layer, param, min, max}`).

**Layer:** `id`, `name`, `effect {id, version}`, `params` (validated against the effect's JSON Schema; the `audio-visualizer` and post-processing effects' are below), `region`, `blend`, `axis`, `enabled` (default true), `solo` (default false), `lane` (UI row: −1…15, −1 places it automatically).
- `region`: `{kind: cells, start, end}` (absolute cells, 0–1999), `{kind: relative, start, end}` (0–1 along a strip) or `{kind: rect, x, y, width, height}` (0–1 of a canvas). Default: the whole surface (`relative`, 0–1).
- `blend`: `max` (default, today's behaviour), `add`, `alpha` or `multiply`.
- `axis`: how a 1D effect runs on a canvas: `linear_x` (default), `linear_y`, `radial` or `angular`.

**Audio visualizer parameters** (today's segment fields, unchanged):

| Field | Range / values | Default |
|---|---|---|
| `freq_lo`, `freq_hi` | 10–24000 Hz; `freq_hi` ≥ `freq_lo` × 1.01 | 60, 250 |
| `trigger` | `level`, `hits`, `beat` | `level` |
| `beats` | 0.5, 1, 2, 4 (the beat trigger flashes every this many beats, aligned to the bar) | 1 |
| `threshold` | level and beat: −120…−1 dBFS; hits: 0…39 dB above the 1 s average | −60 / 3 |
| `ceiling` | level and beat: −119…0 dBFS; hits: 1…40 dB; at least `threshold` + 1 | −25 / 10 |
| `gain` | 0.1–8 | 1 |
| `curve` | 0.2–5 | 1 |
| `attack` | 0–2000 ms | 15 |
| `release` | 0–5000 ms | 200 |
| `floor` (idle glow) | 0–1 | 0 |
| `mode` | fill, pulse, spectrum, shift, scroll, drops, scan, wipe, flow, fire | fill |
| `direction` | forward, reverse, center, edges | forward |
| `speed` | 0–2000 elements/s (moving modes) | 120 |
| `width` | 1–100 % of the region (moving modes) | 20 |
| `peak` | boolean (fill) | false |
| `palette` | 1–16 colours `#RRGGBB` | `["#FFFFFF"]` |

**Post-processing parameters** (the effects that change what is below them in the stack, [pipeline](../architecture/pipeline.md#effects)):

| Effect | Field | Range / values | Default |
|---|---|---|---|
| `brightness` | `brightness` | 0–4 (a multiplier) | 1 |
| `brightness` | `saturation` | 0–4 (0 grey, 1 unchanged) | 1 |
| `mirror` | `keep` | `first`, `second` | `first` |
| `blur` | `radius` | 1–16 cells | 1 |
| `trail` | `fade` | 10–5000 ms | 300 |

**Limits carried over:** 64 layers per surface, 16 lanes, 2000 elements per strip surface, 60 saved presets, 16 tags of up to 24 characters.

**Source** (in `sources.json`): `id`, `name`, `role` (`owner` or `consumer`: this controller's relation to the source), `public_key` (base64url), `surfaces` (assigned surface ids) and `inputs`:

| Field | Range / values | Default |
|---|---|---|
| `audio.device` | `default` (follows the default output), an output name (loopback), or `in:<input name>` | `default` |
| `audio.app` | An app's executable name, lowercase; empty for none (Windows only in v1) | empty |
| `audio.app_mode` | `only`, `except` | `only` |
| `audio.gain` | −24…24 dB | 0 |
| `audio.auto_level` | boolean | false |
| `media.enabled` | boolean: read media sessions (Windows) or MPRIS (Linux) | true |
| `screens[]` | Screen inputs with `enabled` and the selected screen id (one active in v1) | none |

`audio.device` is up to 300 characters and `audio.app` up to 260, as today.

**Provider instance:**
- `id`, `kind` (`hyperion | wled | hue`), `name`, `enabled` (default true), `rate_hz` (1–100, default 40)
- `settings`, by kind. It carries the same `kind`; settings of another kind are replaced by the defaults for the instance's kind.

| Kind | Settings | Defaults |
|---|---|---|
| Hyperion | `host` (up to 255 characters), `port` (1–65535), `priority` (1–253), `origin` (up to 20 characters, [Hyperion's limit](../integrations/hyperion.md)) | `localhost`, 19444, 100, `FOLO` |
| WLED | `host`, `transport` (`ddp` or `dnrgb`), `timeout_s` (1–255, the [realtime timeout](../integrations/wled.md) in seconds) | empty, `ddp`, 2 |
| Hue | `bridge_id` (up to 32 characters), `address`, `streaming_mode` (`managed` or `linked`) | empty, empty, `managed` |

- `output`: the output interface, `element` (default); `raster` is reserved.

**Client** (in `clients.json`): `id`, `name`, `scope` (`control` (default) or `admin`), `key_hash` (the client key's hash; the key itself is never stored), `created` and `last_used` (Unix seconds; 0 for never).

**Secrets** (`secrets.json`): `identity {public_key, private_key}` (X25519, base64url), `master_key_hash`, and `providers {instance id: {app_key?, client_key?, pin?, token?}}` for Hue, WLED and Hyperion credentials.

**Node settings** (`node.toml`, described by `node.json`):

| Field | Range / values | Default |
|---|---|---|
| `role` | `controller`, `source` | `controller` |
| `listen.address` | an IP address or host name | `0.0.0.0` |
| `listen.port` | 1–65535 | 8777 |
| `data_dir` | a path; empty for the platform default ([ADR-0010](../decisions/ADR-0010-settings-persistence-and-migration.md)) | empty |
| `remote_access.enabled` | boolean | false |
| `remote_access.allowed_hosts` | host names and addresses for the Host check, up to 32 | none |
| `log_level` | `error`, `warn`, `info`, `debug`, `trace` | `info` |
| `local_source.enabled` | boolean | true |

## Clamping rules

Python (the effect engine) and Rust (the node) clamp documents with the same algorithm, driven by the schemas. A shared corpus checks that they agree.

- **Structural errors** reject a document: its root isn't a JSON object, or its `schema_version` is newer than the implementation reads. A missing `schema_version` counts as the current one.
- **Objects:** properties the schema doesn't define are dropped. A missing property, or one whose JSON type is wrong, takes its default; a missing object is built from its properties' defaults.
- **Numbers** must be JSON numbers (booleans and strings are the wrong type). Integer fields round to the nearest integer, ties to even. Values outside the range are clamped to it.
- **Strings** longer than `maxLength` are cut to it (counting code points). Colours must match `#RRGGBB` and are upper-cased; anything else takes the default.
- **Enums and booleans:** a value outside the list, or anything but `true` or `false`, takes the default. For example, `beats` other than 0.5, 1, 2 or 4 becomes 1, as `sanitize_segment()` does today.
- **Arrays:** items are clamped one by one. Items of the wrong type are dropped. Items past `maxItems` are dropped. An array that ends up shorter than its `minItems` takes its default.
- **Maps** (`layers_by_role`, `providers` in secrets): each value is clamped.
- **Tagged unions** (`kind`): an unknown kind takes the default of the union; geometry and provider settings follow their owner's `kind`.
- **Ids:** missing, empty or duplicate ids within a list get a new id.
- **Cross-field rules** (`x-folo-rules` in the schemas), as `sanitize_segment()` does today:
  - `band`: `freq_lo` and `freq_hi` are swapped when reversed; `freq_hi` is raised to `freq_lo` × 1.01 (at most 24000, with `freq_lo` then set to `freq_hi` / 1.01); both are rounded to 2 decimals.
  - `thresholds`: `threshold` and `ceiling` take the hits limits and defaults when `trigger` is `hits`, else the level limits; `ceiling` is raised to `threshold` + 1; both are rounded to 1 decimal.
  - `range`: `start` and `end` are swapped when reversed.
- **Params:** a layer's `params` are clamped with its effect's parameter schema when the effect is known (`audio-visualizer`, `brightness`, `mirror`, `blur`, `trail`); otherwise they are kept for the effect to validate.

**Version 1** (today's `studio.json`) keeps its own rules, which are looser about types: `sanitize_config()` in `folo_source/legacy/v1.py` is the reference, `specs/schema/studio-v1.json` describes its limits, and the node's Rust port (`folo-protocol`, module `v1`) must give the same output on the RB-01 corpus.

## Migration from the current `studio.json` (version 1)

Performed by the effect engine's migration tool (RB-05):

| v1 | v2 |
|---|---|
| `led_count`, `segments[]` | Zone "Default", type `custom`; strip surface "Main strip" with resolution = `led_count`; each segment becomes an audio-visualizer layer with region = [start, end] |
| `hyperion {host, port, priority, fps}` | Hyperion provider instance (`enabled` carried over), plus a line target with `led_count` elements and a manual 1:1 mapping, identity adapters |
| `master`, `background` | Surface `master` and `background` |
| `audio.device`, `audio.app`, `audio.app_mode`, `audio.gain`, `audio.auto_level` | Local source audio input settings |
| `reactive`, `ambient.{color, style, level, idle_after}` | Zone "Default" `reactive` and `ambient` |
| `beat.offset` | Zone "Default" `beat_offset_ms` |
| `now_playing.enabled` | Local source `media.enabled` |
| `now_playing.cover_colors` | Zone "Default" `album_colors` |
| `now_playing.pause_ambient` | Zone "Default" `ambient.on_pause` |
| Segment `trigger` `beat`, `beats` | Layer parameters, unchanged |
| `presets[]` | Scenes in the local "My presets" pack, keeping their `led_count` for scaling |
| `default_preset` | Default scene id; built-in ids resolve through the alias table |

The migration keeps a backup (`studio.json.v1.bak`) and can be run more than once with the same result.
