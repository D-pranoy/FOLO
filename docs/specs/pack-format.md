# Pack format

Status: Draft · Implementation: Not started

Decided in [ADR-0021](../decisions/ADR-0021-content-packs.md). The model is in [content](../architecture/content.md).

## Container

A `.folopack` file is a zip archive containing:

```
folo-pack.json          manifest
palettes/*.json
reactions/*.json
looks/*.json
scenes/*.json
zone-templates/*.json
previews/*              optional images or animations
```

## Manifest (`folo-pack.json`)

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | `author/name`, lowercase, `[a-z0-9-]` |
| `version` | yes | Semantic version |
| `folo` | yes | Compatible FOLO version range, for example `>=1.0 <2` |
| `name`, `description` | yes | Display text |
| `license` | yes | An SPDX identifier |
| `authors` | yes | List of names |
| `tags` | no | Up to 16 tags |
| `requires` | no | `{ "author/pack": "^1.2" }` |
| `contents` | yes | Item ids by type |
| `files` | yes | `{ path: sha256 }` for every file |
| `previews` | no | Preview file per item |

## Items

- Each item has `id` (unique within the pack), `schema_version` and `name`. Items refer to each other as `author/pack:item`, with an optional version range.
- **Scenes** contain `layers_by_role`, `macros` and optional `fallbacks`. They never contain addresses or keys.
- **Zone templates** contain targets with geometry and capabilities, but no provider bindings.
- **Validation** uses the same clamping rules as the [configuration model](config-model.md). Packs that fail structural validation are rejected as a whole.
- **Limits:** 10 MB per pack, 500 items per pack.

## Built-in packs

`folo-core`, `folo-genres` and `folo-effects` ship with the node and can't be removed. User presets live in the local, editable `local/my-presets` pack.
