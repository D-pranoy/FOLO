# Pack format

Status: Accepted · Implementation: Partial (JSON Schemas for the manifest and the items, and the built-in packs in `packs/folo/`; nothing reads packs yet)

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

- Each item is a file `<folder>/<name>.json`. The name is free within the rule below (the migration writes `scenes/preset-NNN.json`, and the node names new items `item-NNN.json`); the item's `id` field identifies it, and `contents` lists the ids of each folder's files.
- **Names:** folders are flat. Every entry is `folo-pack.json`, `<folder>/<name>.json` or `previews/<name>.<png|jpg|webp|gif>`, where `<name>` matches `[a-z0-9][a-z0-9_-]{0,63}` and isn't one of Windows' reserved names (`con`, `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9`). Directory entries are ignored; nothing else may be in the archive.
- Entries are stored or deflated, never encrypted; none appears twice and none is a link.
- **Limits:** the archive at most 10 MB; at most 1,000 entries and 32 MB uncompressed in all, counted on the bytes decompressed; the manifest and each item file at most 1 MB; at most 500 items, all types counted; previews PNG, JPEG, WebP or GIF (by content), at most 2 MB each. A local pack holds at most 60 scenes in `local/my-presets`. The library holds at most 100 packs and 128 MB.

## Manifest (`folo-pack.json`)

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | `author/name`, lowercase, `[a-z0-9-]`, 1–32 characters each. The authors `folo` (the built-in packs) and `local` (the node's own packs) are reserved: nothing installs under them. |
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
| `categories` | no | Up to 32 gallery groups: `id`, `name`, `description` and `items` (item references, in order). Clients merge them by id across packs. The category `mine` lists the scenes of `local/*` packs by rule, and clients put scenes no category lists under "More". |

**Version ranges** (`folo` and the values of `requires`) are read as Rust's `semver` crate reads a [`VersionReq`](https://docs.rs/semver/1/semver/struct.VersionReq.html), with spaces between comparators read as commas: `>=1.0 <2` is `>=1.0, <2`. The node's version is compared without its pre-release part. `folo` isn't checked for `local/*` packs, nor by a development build of the node (version `0.0.0`).

## Items

- Each item has `id` (1–64 characters, unique within the pack across types; missing or empty is a structural error), `schema_version`, and optionally `description` (up to 500 characters, shown in the gallery) and `hints` (an object for clients, such as the tempo, a preview animation, a group or `source: "LedFx"`, which the node and the engines ignore). Palettes, reactions and looks have a `name`; a scene's name is its `scene.name`.
- **References:** items refer to each other, and the API and `show.json` refer to them, as `author/pack:item`, at most 130 characters, split at the first `/` and then the first `:`. In a URL path a reference or a pack id is percent-encoded (`folo%2Fcore`). How a reference, an alias and a bare id are told apart is in [ADR-0038](../decisions/ADR-0038-content-library.md) D2.
- **Scenes** contain a scene of the [configuration model](config-model.md#entities-summary) (`layers_by_role`, `macros`), optional `fallbacks` (role: another role to try), and `led_count` when their `cells` regions were made for a strip of that length (0 otherwise), so they can be scaled to another. They never contain addresses or keys.
- **Zone templates** contain a zone with targets, geometry and capabilities, but no provider bindings. Clients create a zone from one with `POST /zones`.
- **Looks and reactions** are copied into a layer by clients.

## Validation

[ADR-0038](../decisions/ADR-0038-content-library.md) D4 to D8. A pack is checked as a whole before anything is written:

- **Structural errors** reject it (`422 structural_error`, with the path and the reason): a container rule above broken; the manifest missing, not a JSON object, or a required field missing or of the wrong JSON type; an `id` outside the pattern or with a reserved author; a `version` that isn't a semantic version; `files` not listing exactly the other entries, or a hash that doesn't match; `contents` not listing exactly the ids of each folder's files; an item that isn't a JSON object or whose `schema_version` is newer than the node reads; two items with one id; a preview of an unknown item or file.
- **Compatibility:** a `folo` range the node doesn't meet gives `422 incompatible`; a `requires` entry not installed in a matching version gives `422 missing_dependency`.
- **Clamping:** every item is then clamped with its type's schema, as documents are ([configuration model](config-model.md#clamping-rules)), and so are the manifest's other fields. Ids the clamper must make are the first 8 hexadecimal characters of the SHA-256 of `<pack id>:<item id>:<n>`, `n` counting from 1, so a pack always clamps alike.
- **Canonical JSON and hashes:** an item's canonical JSON is its clamped value serialised by the JSON Canonicalization Scheme ([RFC 8785](https://www.rfc-editor.org/rfc/rfc8785)). An item's **hash**, which provenance records, is the SHA-256 of its canonical JSON, whichever program wrote its file.
- **Canonical form:** the library keeps the clamped items with a manifest whose `files` lists their files' hashes (files on disk may be indented). Exports are that form, zipped deterministically: `folo-pack.json` first and then the other entries in byte order of their paths, each item as its canonical JSON with `files` rewritten to match, deflated, dated 1980-01-01 00:00, without extra fields or directory entries. The **pack hash** is the SHA-256 of that archive.

## Built-in packs

`folo/core`, `folo/genres` and `folo/effects` are embedded in the node and can't be removed or edited ([ADR-0038](../decisions/ADR-0038-content-library.md) D1, D17). Their contents and the alias table for the studio's ids (`fx:*`, `genre:*`) are in [content](../architecture/content.md#built-in-library).

User presets live in the local, editable `local/my-presets` pack, which the node creates when it is missing. Every pack whose author is `local` is editable item by item.
