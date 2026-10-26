# Pack format

Status: Accepted · Implementation: Partial (JSON Schemas for the manifest and the items; nothing reads packs yet)

Decided in [ADR-0021](../decisions/ADR-0021-content-packs.md) and [ADR-0038](../decisions/ADR-0038-content-library.md). The model is in [content](../architecture/content.md), and the library's routes in the [client API](client-api.md#library).

## Container

A `.folopack` file is a zip archive containing:

```
folo-pack.json          manifest
palettes/*.json
reactions/*.json
looks/*.json
scenes/*.json
zone-templates/*.json
previews/*              optional images
```

- Each item is a file `<folder>/<name>.json`. The name is free (the migration writes `scenes/preset-NNN.json`, and the node names new items `item-NNN.json`); the item's `id` field identifies it, and `contents` lists the ids of each folder's files. Nothing else may be in the archive.
- Entries are stored or deflated, never encrypted. Their paths are relative, UTF-8, use `/`, and contain no `..`, backslash, drive letter or NUL; none appears twice and none is a link.
- **Limits:** the archive at most 10 MB; at most 1,000 entries, each at most 10 MB and all together at most 32 MB uncompressed; an item file at most 1 MB; at most 500 items, all types counted; previews PNG, JPEG, WebP or GIF (by content), at most 2 MB each. A local pack holds at most 60 scenes in `local/my-presets`.

## Manifest (`folo-pack.json`)

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | `author/name`, lowercase, `[a-z0-9-]`, 1–32 characters each. The author `folo` is reserved for the built-in packs. |
| `version` | yes | Semantic version |
| `folo` | yes | Compatible FOLO versions, for example `>=1.0 <2` (below) |
| `name`, `description` | yes | Display text |
| `license` | yes | An SPDX identifier |
| `authors` | yes | List of names |
| `tags` | no | Up to 16 tags |
| `requires` | no | `{ "author/pack": "^1.2" }` |
| `contents` | yes | Item ids by type: `palettes`, `reactions`, `looks`, `scenes`, `zone_templates` |
| `files` | yes | `{ path: sha256 }` for every other file, in lowercase hexadecimal |
| `previews` | no | Preview file per item id |
| `categories` | no | Up to 32 gallery groups: `id`, `name`, `description` and `items` (item references, in order). Clients merge them by id across packs. |

**Version ranges** (`folo` and the values of `requires`) are read as Rust's `semver` crate reads a [`VersionReq`](https://docs.rs/semver/1/semver/struct.VersionReq.html), with spaces between comparators read as commas: `>=1.0 <2` is `>=1.0, <2`. A development build of the node (version `0.0.0`) doesn't check `folo`.

## Items

- Each item has `id` (unique within the pack, across types), `schema_version`, and optionally `description` (up to 500 characters, shown in the gallery) and `hints` (an object for clients, such as the tempo, a preview animation, a group or `source: "LedFx"`, which the node and the engines ignore). Palettes, reactions and looks have a `name`; a scene's name is its `scene.name`.
- **References:** items refer to each other, and the API and `show.json` refer to them, as `author/pack:item`. In a URL path a reference or a pack id is percent-encoded (`folo%2Fcore`).
- **Scenes** contain a scene of the [configuration model](config-model.md#entities-summary) (`layers_by_role`, `macros`), optional `fallbacks` (role: another role to try), and `led_count` when their `cells` regions were made for a strip of that length (0 otherwise), so they can be scaled to another. They never contain addresses or keys.
- **Zone templates** contain a zone with targets, geometry and capabilities, but no provider bindings. Clients create a zone from one with `POST /zones`.
- **Looks and reactions** are copied into a layer by clients.

## Validation

[ADR-0038](../decisions/ADR-0038-content-library.md) D4 to D8. A pack is checked as a whole before anything is written:

- **Structural errors** reject it (`422 structural_error`, with the path and the reason): a container rule above broken; the manifest missing, not a JSON object, or a required field missing or of the wrong JSON type; an `id` outside the pattern; a `version` that isn't a semantic version; `files` not listing exactly the other entries, or a hash that doesn't match; `contents` not listing exactly the ids of each folder's files; an item that isn't a JSON object or whose `schema_version` is newer than the node reads; two items with one id; a preview of an unknown item or file.
- **Compatibility:** a `folo` range the node doesn't meet gives `422 incompatible`; a `requires` entry not installed in a matching version gives `422 missing_dependency`.
- **Clamping:** every item is then clamped with its type's schema, as documents are ([configuration model](config-model.md#clamping-rules)), and so are the manifest's other fields.
- **Canonical form:** the library keeps the clamped items with a manifest whose `files` lists their hashes. Exports are that form, zipped deterministically: entries in byte order of their paths with `folo-pack.json` first, deflated, dated 1980-01-01 00:00, without extra fields. The **pack hash** is the SHA-256 of that archive.

## Built-in packs

`folo/core`, `folo/genres` and `folo/effects` are embedded in the node and can't be removed or edited ([ADR-0038](../decisions/ADR-0038-content-library.md) D1, D17). Their contents and the alias table for the studio's ids (`fx:*`, `genre:*`) are in [content](../architecture/content.md#built-in-library).

User presets live in the local, editable `local/my-presets` pack, which the node creates when it is missing. Every pack whose author is `local` is editable item by item.
