# Configuration model

Status: Draft · Implementation: Not started

The controller's persistent model. Validation **clamps values to their limits and never rejects** a document, except for structural errors. Hand-written or stale input therefore always results in a working configuration, matching today's `sanitize_config()` behaviour.

## Documents

Each document has `schema_version`, `revision` and `server_id`. Writes are atomic (temporary file, fsync, replace) and debounced.

| Document | Contents |
|---|---|
| `node.toml` | Host settings: role, listen address and port, data paths, remote access, log level. Overridable by `FOLO_*` and CLI flags ([control plane](../architecture/control-plane.md#settings-layering)). |
| `zones.json` | Zones, targets, surfaces, mappings |
| `show.json` | Active scene per zone; scenes; layers; master; background |
| `providers.json` | Provider instances (Hyperion, WLED, Hue) and their settings |
| `sources.json` | Paired sources, their owner or consumer role, assigned surfaces, inputs and their settings |
| `clients.json` | Client keys (stored hashed), names, scopes, last use |
| `library/` | Installed packs and the local "My presets" pack |
| `secrets.json` | Provider credentials, identity keys. Owner-only permissions; never exported or returned. |

## Entities (summary)

**Zone:** `id`, `name`, `type` (`music | screen | desk | room | custom`), `bounds {x, y, z}` in metres, `reference {kind, position, orientation}`, `targets[]`, `surfaces[]`, `mappings[]`, `flash_limiter {enabled, exempt_reason?}`, `hue_sync {mode, configuration_id?}`, plus:

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

**Target:**
- `id`, `name`
- `kind`: `point | line | grid | display`, where display is reserved and never executed
- `provider {instance, address}`
- `geometry`, by kind:
  - point: position
  - line: polyline + count, or explicit positions
  - grid: origin, u, v, w, h, wiring
  - display: rect, resolution, warp, mask
- `capabilities` (discovered plus overrides)
- `calibration {gamma, white_balance, brightness_min, brightness_max, color_order, latency_ms}`

**Surface:** `id`, `kind` (`strip | canvas`), `resolution`, `placement` (path, plane or none), `role`, `background`, `master`.

**Mapping:** `surface`, `target`, `method` (`plane | path | manual`), `kernel` (`nearest | bilinear | area`, radius), `edge` (`clamp | none`), `priority`, `blend`, plus manual ranges.

**Scene:** `id`, `name`, `tags`, `provenance?`, `layers_by_role {role: Layer[]}`, `macros[]`.

**Layer:** `id`, `effect {id, version}`, `params` (validated against the effect's JSON Schema), `region` (cell range or rectangle), `blend`, `enabled`, `solo`, `lane` (UI row).

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

**Limits carried over:** 64 layers per surface, 16 lanes, 2000 elements per strip surface, 60 saved presets, 16 tags of up to 24 characters.

**Source inputs** (in `sources.json`, per source):

| Field | Range / values | Default |
|---|---|---|
| `audio.device` | `default` (follows the default output), an output name (loopback), or `in:<input name>` | `default` |
| `audio.app` | An app's executable name, lowercase; empty for none (Windows only in v1) | empty |
| `audio.app_mode` | `only`, `except` | `only` |
| `audio.gain` | −24…24 dB | 0 |
| `audio.auto_level` | boolean | false |
| `media.enabled` | boolean: read media sessions (Windows) or MPRIS (Linux) | true |
| `screens[]` | Screen inputs with `enabled` and the selected screen id (one active in v1) | none |

**Provider instance:**
- `id`, `kind` (`hyperion | wled | hue`), `name`, `enabled`, `rate_hz`
- `settings`, by kind:
  - Hyperion: host, port, priority, origin
  - WLED: host, transport (`ddp | dnrgb`), timeout
  - Hue: bridge id, address, streaming mode (`managed | linked`)
- The output interface is `element`; `raster` is reserved.

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
