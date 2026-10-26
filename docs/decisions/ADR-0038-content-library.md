# ADR-0038: The content library and the built-in packs

Status: Proposed
Date: 2026-10-26

## Context

RB-09 turns content into data: installed packs, scenes applied to zones by role, the studio's built-in looks as packs, Reset all on the controller, and backup and restore ([RB-09](../runbooks/m3-controller-core.md#rb-09-content-library)). [ADR-0021](ADR-0021-content-packs.md) fixes the model (data-only packs, roles, copies with provenance), [ADR-0023](ADR-0023-legacy-code-and-licensing.md) D3 the clean-room rewrite of the LedFx-inspired content, and [ADR-0035](ADR-0035-serving-the-client-api.md) D2 puts backup and restore here. The documents leave open what an implementation has to choose:

- **The library service:** where packs live, what install, list, remove and export do, which errors they give, how big a pack may be, what happens when an id is taken, and how a pack id travels in a URL (it contains a `/`, and the API's paths take `{id}`).
- **Applying:** which scene a zone gets, what the copy carries, how roles are matched to a zone's surfaces, where a user's choice of surface is remembered, and what "re-apply, keeping my changes" merges against when the old version is gone.
- **The built-ins:** today they are JavaScript builders in `web/studio.js` (`GENRE_PRESETS`, `EFFECTS`, `LEDFX_EFFECTS`, `REACTIVE`, `PRESETS`, `CATEGORIES`, `layoutRanges()`, `part()`), exported for the goldens at 30, 61 and 144 LEDs by `scripts/export_builtins.mjs` into `tests/golden/builtins.json`. Nothing says how they become data that gives the same LEDs at every length.
- **Reset all** runs in the browser today for a default preset, and in `/api/reset` otherwise ([current system](../architecture/current-system.md)).
- **Distribution:** [content](../architecture/content.md#distribution) says the controller pushes packs to sources, checked by hash, and the [source link](../specs/source-link.md) has `packs.install` and `packs.remove`, but no document says what a source does with a pack.
- **The rewrite gate** is a line in the release checklist, with nothing that tracks it.
- **Backup and restore** have routes in `specs/openapi.yaml` and no format.

Facts that shape the answer:

- **The builders round to the nearest LED; the engine doesn't.** `part()` and `layoutRanges()` place a segment from fraction `f` to `t` of an `n`-LED strip at `round(f·n)` to `max(start, round(t·n) − 1)` (JavaScript's `Math.round`, halves up). The engine resolves a `relative` region to `⌊f·N⌋` to `⌈t·N⌉ − 1` ([pipeline](../architecture/pipeline.md#surfaces-and-layer-stacks)). Measured over the 46 built-ins at every length from 1 to 2000 LEDs, the engine's rule places 146,078 of 352,000 segments differently; the builders' rule, written as data, places all of them exactly.
- **An applied scene is self-contained.** `config.apply` sends each source the full scenes of its surfaces, layers and parameters included ([source link](../specs/source-link.md#controller--source-json-rpc-methods)), and v1 packs hold data only. A source never needs a pack to render.
- **The engine picks layers by role.** It renders, for each surface, the layers its zone's active scene lists under that surface's `role` (`layers_by_role[surface.role]`, `runtime/executor.py`).
- **The migration already writes a pack:** `library/local/my-presets/` in the data folder, a manifest and `scenes/preset-NNN.json` ([configuration model](../specs/config-model.md#migration-from-the-current-studiojson-version-1)), and `default_scene` keeps a saved preset's bare id or a built-in id (`fx:*`, `genre:*`).
- **Lengths that don't fit.** A pack id is up to 65 characters (`author/name`, 32 each), but `Scene.provenance.pack` is limited to 64. An item reference (`author/pack:item`) is up to 130, but `default_scene` is a 64-character `Ref`.

## Decision

**Where packs live**

- **D1.** **Three places, one version each.**
  - **Built-in packs** (`folo/core`, `folo/genres`, `folo/effects`) are committed in `packs/folo/<name>/` and embedded in the node at build time, as the UI is ([ADR-0035](ADR-0035-serving-the-client-api.md) D7). They are never written to disk, can't be removed or edited, and change only with the node.
  - **Installed and local packs** live unpacked in the data folder, at `library/<author>/<name>/`: `folo-pack.json`, the item files and the previews, as the migration already writes `local/my-presets` ([control plane](../architecture/control-plane.md#folders)).
  - A pack id is installed in one version at a time. `local/my-presets` is created empty when it is missing.
- **D2.** **Ids.** A pack id is `author/name` (`[a-z0-9-]`, 1–32 characters each). The author `folo` is reserved for the built-in packs. An item is referred to as `author/name:item`, its **reference**. In a URL path a pack id or a reference is percent-encoded, so `folo/core` is `folo%2Fcore` ([RFC 3986, section 2.1](https://www.rfc-editor.org/rfc/rfc3986#section-2.1)); the node decodes the segment after routing.
- **D3.** **Reading the library at start.** The node validates every pack folder as an install does (D4, D5). A folder that fails is left as it is, skipped, and logged as an error with its path and the reason; the node starts anyway, because packs are content, not configuration ([ADR-0034](ADR-0034-the-node-process.md) D11 still holds for the documents).

**Installing, listing, removing, exporting**

- **D4.** **Structural validation** rejects a pack as a whole, with `422 structural_error` and the offending path and reason in `details`:
  - the body isn't a zip archive, or uses anything but the stored and deflate methods, encryption, or more entries or bytes than D6 allows
  - an entry's path is absolute, contains `..`, a backslash, a drive letter or a NUL, isn't valid UTF-8, appears twice, is a link, or lies outside `folo-pack.json`, the five item folders and `previews/`
  - `folo-pack.json` is missing or isn't a JSON object, or one of its required fields ([pack format](../specs/pack-format.md#manifest-folo-packjson)) is missing or of the wrong JSON type; `id` doesn't match the pattern; `version` isn't a semantic version
  - `files` doesn't list exactly the archive's entries other than `folo-pack.json`, or a hash doesn't match its entry's bytes
  - an item file (any `<folder>/<name>.json`) isn't a JSON object or its `schema_version` is newer than the node reads; two items of the pack share an `id`; or `contents[type]` doesn't list exactly the ids of the files in that type's folder. File names don't follow ids, which may hold any character (the migration writes `scenes/preset-NNN.json`)
  - a `previews` entry names an item the pack doesn't have, or a file it doesn't contain
- **D5.** **Clamping and the canonical form.** Every item is then clamped with its type's schema, like a document ([configuration model](../specs/config-model.md#clamping-rules)), and so are the manifest's other fields. The node writes the clamped items and a manifest whose `files` lists their new hashes. That **canonical form** is what the library holds, lists, exports and hashes.
- **D6.** **Limits:**
  - the archive at most 10 MB (the install route's body limit), at most 1,000 entries, each at most 10 MB and all together at most 32 MB uncompressed
  - an item file at most 1 MB; at most 500 items per pack, all types counted
  - previews PNG, JPEG, WebP or GIF, by their content, at most 2 MB each
  - a local pack's scenes: at most 60 in `local/my-presets`, as today's saved presets
- **D7.** **Compatibility.** `folo` and the ranges in `requires` are version requirements as Rust's `semver` crate reads them ([`VersionReq`](https://docs.rs/semver/1/semver/struct.VersionReq.html)), with spaces between comparators read as commas (`>=1.0 <2` is `>=1.0, <2`). A pack whose `folo` the node's version doesn't meet gets `422 incompatible`; one whose `requires` names a pack that isn't installed, or installed in a version the range excludes, gets `422 missing_dependency` with those packs in `details`. A development build (version `0.0.0`) skips the `folo` check and logs a warning.
- **D8.** **Conflicting ids.**
  - A pack whose author is `folo` gets `409 reserved_id`.
  - A pack with the id of an installed pack replaces it when its version is **newer**. The same or an older version gets `409 already_installed` with both versions in `details`, unless the request carries `replace=true`, which installs it anyway.
  - Items of different packs never conflict: references carry their pack. Scenes already applied are copies and stay as they are (D11); their provenance tells the UI that the pack changed.
- **D9.** **Removing and exporting.**
  - `DELETE` of a built-in pack or `local/my-presets` gets `409 protected_pack`; of a pack another installed pack requires, `409 required_by` naming them.
  - Export answers the canonical form as a `.folopack`: entries in byte order of their paths, `folo-pack.json` first, each deflated with the date 1980-01-01 00:00 and no extra fields. The **pack hash** is the SHA-256 of that archive. It is the same for the same content on one node build, which is all its uses need.
- **D10.** **Editing local packs.** Packs whose author is `local` are editable through their items: `PUT` writes an item, clamped, into its existing file or, for a new item, into `<folder>/item-NNN.json` with the lowest free number, and updates the manifest's `contents` and `files`; `DELETE` removes it. Items of other packs are read-only (`409 read_only_pack`). Saving a preset is a `PUT` of a scene item to `local/my-presets`.

**Applying**

- **D11.** **What an apply writes.** `POST /scenes/{ref}/apply` with `{zone, roles, keep_changes}`:
  - **What it copies:** a reference (`author/pack:item`), an alias (D19) or a scene of `show.json` by its id.
  - **Where the copy goes:** into the zone's active scene, **in place** (same id and position), when no other zone has that scene active. Otherwise into a new scene with a new id, appended, which then becomes the zone's active scene. `show.json`'s limit of 256 scenes gives `409 list_full`.
  - **What the copy holds:** the item's name and tags, its macros, its layers bound to the zone's surfaces (D12) and scaled (D14), and `provenance {pack, version, item, hash}`, where `hash` is the SHA-256 of the item's file in the canonical form. Copying a scene of `show.json` copies its provenance.
  - **The base for later merges:** the node keeps the copy as written in `applied/<scene id>.json` in the data folder, and removes that file when the scene leaves `show.json`.
  - **The write:** one write of `show.json` (with `If-Match`, when sent), so the document gets its next revision, `config_changed` goes out, and the engine gets the new configuration. The previous content is not kept: undo is the client's, by writing back the show it had.
- **D12.** **Role binding**, for each role of the scene in its order, against the zone's surfaces, each surface bound at most once:
  1. **An override:** the surface the request's `roles` names, else the one remembered for this scene and zone (D13), when it exists in the zone.
  2. **The same role:** the first surface, in `zones.json` order, whose `role` equals it.
  3. **The same kind:** for a role `strip.<name>` a strip surface, for `canvas.<name>` a canvas surface; among those, the one whose cell count is nearest the scene's `led_count` (when it is not 0), else the one with the most cells; ties in `zones.json` order. Other prefixes (`points.<name>` and the like) match only by steps 1 and 2.
  4. **Capability:** a surface qualifies in steps 2 and 3 only when it has at least as many cells as the largest `min_elements` among the role's layers' effects in the catalog.
  5. **Fallbacks:** the item's `fallbacks[role]`, bound by steps 1 to 4; then, for a `strip.*` role, the surface with role `canvas.room`, else the zone's first canvas surface. Strip layers on a canvas run along `linear_x`: their regions select columns over every row ([pipeline](../architecture/pipeline.md#surfaces-and-layer-stacks)).
  6. **No match:** the role is skipped, and the answer lists it.

  The copy lists each bound role's layers under **the bound surface's** `role`, because that is how the engine finds them. Surfaces that share a role therefore share layers. Surfaces have no tags: the name after a role's dot plays that part.
- **D13.** **Remembered overrides** live in `show.json`, in `bindings`: a list of `{zone, scene, roles: {role: surface id}}`, at most 256, where `scene` is the reference as resolved (an alias becomes its pack reference). An apply whose `roles` names surfaces stores them for that scene and zone, merged into what was there; a role given as `""` returns to automatic and is removed; an entry with no roles left is removed. Entries whose zone no longer exists are dropped at the next write of `show.json`.
- **D14.** **Scaling and regions.**
  - A layer region `relative` gains `snap`: `cover` (today's rule, also when `snap` is absent) or `nearest`, which places the region at `min(R(start × N), N − 1)` to `min(max(first, R(end × N) − 1), N − 1)` cells, where `R(x)` is `⌊x⌋ + 1` when `x − ⌊x⌋ ≥ 0.5` and `⌊x⌋` otherwise (halves up, as JavaScript's `Math.round`). The built-in scenes use it (D17).
  - `cells` regions of a scene whose `led_count` is not 0 are scaled to the bound surface's width `W` when it differs, as the studio scales saved presets today (`scaleSegments()`): with `r = W / led_count`, `start' = min(W − 1, R(start × r))` and `end' = min(W − 1, max(start', R((end + 1) × r) − 1))`.
  - `rect` regions and `relative` ones aren't changed.
- **D15.** **Re-apply, keeping my changes:** `keep_changes: true` merges into the zone's active scene, which must carry provenance of the same pack and item and have its `applied/` file, else `409 no_base`. **Base** is the `applied/` file, **mine** the scene as it is now, **theirs** the item as an apply would write it now, with the same bindings. Then, value by value:
  - Layers are matched by id within each role, and macros by id. Every field of a layer, every key of its `params` and every field of a macro is a value. A list (a palette, `tags`) is one value.
  - A value that mine left as it was in base takes theirs; a value mine changed keeps mine.
  - A layer only in theirs is added; a layer in base that mine removed stays removed; a layer only in mine is kept. The order is theirs', then mine's own layers in their order.
  - The result becomes the new base.
- **D16.** **Macros** are evaluated by clients: moving a knob writes the parameters it drives (`min + value × (max − min)`) with the scene. The node and the engine don't read `macros`.

**The built-in packs**

- **D17.** **What they hold:**

  | Pack | Items |
  |---|---|
  | `folo/core` | The 9 palettes (`PRESETS`), the 25 reactive presets as reactions (`REACTIVE`, each with its group), and the scene `default`: today's reset segments (bass, mids and highs in thirds) |
  | `folo/genres` | The 14 genre presets, as scenes |
  | `folo/effects` | The 14 effects and the 18 LedFx-inspired effects, as scenes |

  - **Ids:** a scene's item id is its built-in id without the prefix (`genre:edm` is `folo/genres:edm`, `fx:ledfx-bar` is `folo/effects:ledfx-bar`); palettes and reactions take the slug of their name (`deep-night`, `kick-punch`). Ids never change once released; renaming changes names only.
  - **Scenes** have one role, `strip.main`, `led_count` 0, and one layer per builder segment, in the builder's order, with ids `1`, `2`, …: an `audio-visualizer` layer whose region is `relative` with `snap: nearest` and the builder's fractions, and whose parameters are the segment's.
  - **Items gain** `description` (the builders' `desc`; the LedFx-inspired ones say "LedFx-inspired", [ADR-0023](ADR-0023-legacy-code-and-licensing.md) D3) and `hints`, an object for clients that the node and the engines ignore: `tempo`, the preview animation and its length, a reaction's group, and `source: "LedFx"` for the badge.
  - **Categories:** the manifest gains `categories` (id, name, description, references), and `folo/core` carries today's eight (`CATEGORIES`). The seven bands and six colour ranges are editor settings, not content, and stay in the UI.
  - Version `1.0.0`, licence MIT, authors "FOLO contributors"; a change to a pack's items raises its version.
- **D18.** **Generated from the JavaScript, while it exists.** `scripts/export_builtins.mjs --trace` records, for each built-in and each segment, the fractions the builder used and the segment, plus the palettes, reactive presets, categories and descriptions. `scripts/build_packs.py generate` writes `packs/folo/` from it, clamped and in the canonical form; `build_packs.py check` regenerates into a temporary folder and fails on any difference, in CI's contract job. When the UI's switch-over removes `web/` ([RB-11](../runbooks/m4-interface.md)), `packs/folo/` becomes the source and the generator goes.
- **D19.** **The alias table** is `packs/aliases.json`: the 46 built-in ids (`fx:*`, `genre:*`) and their references, embedded in the node with the packs. It never loses an entry. A bare id, in `default_scene` or an apply, names a scene of `show.json`, else `local/my-presets:<id>` (which is what the migration wrote for saved presets).
- **D20.** **Equivalence.** The built-in scenes are equivalent to the builders when:
  - **Frames:** rendered as a `strip.main` surface of 30, 61 and 144 cells, with the [golden](../process/testing-policy.md#golden-files) signals, they reproduce `tests/golden/renderer/builtin-*.png` byte for byte. No golden is regenerated.
  - **Layers:** at every length from 1 to 64 and at 100, 144, 255, 256, 300, 512, 1000, 1024, 1999 and 2000, the scene's layers, clamped and with regions resolved to cells, equal the builder's segments run through `sanitize_config()` and the RB-05 migration's conversion to layers, field for field, ids aside. The builders' output comes from `export_builtins.mjs` run by the test; without Node 22 the test is skipped, and `FOLO_NODE_TESTS=1` (set in CI's contract job) makes that a failure.
  - **Reactions and palettes** equal `REACTIVE` (range and response fields) and `PRESETS`.
  - **Scaling:** a scene with `cells` regions and a `led_count` scales as `scaleSegments()` does, checked against it at the same lengths.

  `folo/core:default` is the exception: its thirds are `relative` with `snap: nearest`, so when the length isn't a multiple of 3 its boundaries round to the nearest LED where today's `default_config()` rounds down (`led_count // 3`): at 32 LEDs, 0–10, 11–20 and 21–31 instead of 0–9, 10–19 and 20–31.
- **D21.** **Reset all** is `POST /show/reset {zones}` (empty: every zone). For each zone it applies `default_scene` as an apply without `roles` does, using the remembered bindings, and sets the `master` of the zone's surfaces to 1 and their `background` to `#000000`, as the studio does today. An empty `default_scene`, or one that resolves to nothing, applies `folo/core:default`, logging a warning in the second case. Zone settings (reactive mode, ambient light, beat offset, album colours) and `default_scene` stay as they are. It writes `show.json` and, when a surface changed, `zones.json`.

**Distribution**

- **D22.** **No packs go to sources in v1.** An applied scene reaches its source whole in `config.apply` (Context), so the set of packs a source needs is empty. `packs.install` and `packs.remove` stay in the contract with their behaviour fixed for the content that will need them ([ADR-0021](ADR-0021-content-packs.md) D6): the controller sends the canonical archive inline (base64) with its pack hash; the source checks the hash before anything else and answers `-32000` with `hash_mismatch` when it differs, keeps packs in its own `library/` as D1 says, and answers `{}`; `packs.remove` of a pack it doesn't have answers `{}`. `url` is reserved. Until then a source answers both with `-32601`, as the [source link](../specs/source-link.md#messages-inside-the-noise-channel) requires for methods it doesn't implement.

**The clean-room rewrite gate**

- **D23.** **Tracking.** [Effect specifications](../specs/effects/README.md) lists every LedFx-inspired item: the six moving modes of the audio visualizer (`scroll`, `drops`, `scan`, `wipe`, `flow`, `fire`) and the 18 LedFx-inspired scenes, each with its specification file and a status: `ported` (today), `specified` (its specification is written) or `rewritten` (reimplemented from it).
  - A specification describes behaviour from observation and from LedFx's documentation, never from LedFx's code or from FOLO's port of it. It goes in `docs/specs/effects/<id>.md`, and its pull request says who wrote it by role (not by name) and that they followed this rule.
  - The rewrite is made by someone (a person or a fresh agent session) given only the specification, never the current code of that mode, and its pull request says so. Its goldens are regenerated, which is a behaviour change a maintainer approves.
- **D24.** **The gate is a test.** `tests/test_clean_room_gate.py` checks that the list names exactly the scenes tagged `LedFx` in the built-in packs and the moving modes in the engine, and that a row past `ported` has its specification file. With `FOLO_RELEASE_GATE=1` it also fails unless every row is `rewritten`. The release workflow ([RB-23](../runbooks/m8-release.md)) sets it for the first public binary release and every one after it; until every row is `rewritten`, no public binary can be built.

**Backup and restore**

- **D25.** **Format.** A backup is a zip archive (`application/zip`, named `folo-backup-<UTC date and time>.zip`) holding `folo-backup.json` and the files it lists. The manifest is `BackupManifest` in the contract: `format` (1), `created` (Unix seconds), `node_version`, `schema_version` (2) and `files {path: sha256}`.
- **D26.** **Contents.**
  - **In:** `zones.json`, `show.json`, `providers.json` and `sources.json`; every installed and local pack, in its canonical form, under `library/`; and the `applied/` files.
  - **Out:** `secrets.json` (identity keys, provider credentials), `clients.json` (client keys belong to one controller: clients pair again), `node.toml` (host settings belong to the machine) and the built-in packs (they come with the node).
- **D27.** **Restore** (`POST /system/restore`, at most 64 MB):
  - **Checks first, as a whole:** the archive as D4 checks a pack (paths, methods, limits of 64 MB and 10,000 entries), the manifest, the hashes, every document clamped without a structural error, and every pack as an install would check it. A failure gets `422 structural_error` (or `422 incompatible` for a newer `format` or `schema_version`), and nothing changes.
  - **Then replace:** the four documents become the backup's, each with the revision after the one it has now and this node's `server_id`; the library's installed and local packs become exactly the backup's; `applied/` likewise. Secrets, clients and `node.toml` stay as they are, so provider credentials and paired sources carry over only on the node that made the backup.
  - **Atomically:** the node writes everything to `restore/` in the data folder with a `restore.pending` marker, then moves each file into place and removes the marker. A node that starts with the marker present finishes the moves first.
  - **Then:** `config_changed` for each document, the new configuration to the engine, and `204`.

**API and scopes**

- **D28.** **Routes.** The library's routes and their errors are in the [client API](../specs/client-api.md#library): `/library/packs` (list, install with `?replace`), `/library/packs/{pack}` (manifest, remove), `/library/packs/{pack}/export`, `/library/packs/{pack}/items/{type}/{item}` (read; write and delete in local packs), `/library/packs/{pack}/previews/{file}`, `/library/items` (every item of a type, with its pack, for the gallery), `/scenes/{ref}/apply`, `/show/reset`, `/system/backup` and `/system/restore`. Install takes `application/zip` up to 10 MB and restore up to 64 MB; every other request keeps the JSON rule and the 1 MB limit. An install, a removal and an item write send `event {kind: library, id: <pack id>, state}` on the live channel, so clients reload the gallery.
- **D29.** **Scopes** (enforced from RB-15): installing, replacing and removing packs, backup and restore need `admin`; reading the library, applying, Reset all and writing items of local packs need `control`.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Built-in scenes with `cells` regions at one length and `led_count` scaling | No new region field | Scaling rounds twice, so it misses the builders at most lengths |
| One scene per built-in per LED count | Exact | Unbounded data, and nothing for lengths not listed |
| Change `relative` to round to the nearest cell | No new field | Changes every existing layer's output, and the goldens with it |
| Built-in packs written to the data folder on first run | One code path with installed packs | They'd drift from the node's version, and a user could edit what an update overwrites |
| Packs kept as `.folopack` files, not unpacked | Export is a copy | Every read unzips; the migration already writes an unpacked folder |
| Reject an install whose id is taken | Simplest | Updating a pack would need a remove first, losing nothing but adding a step that fails halfway |
| Several versions of a pack side by side | Old provenance resolves | Two copies of everything; the `applied/` file already gives the merge its base |
| Store the merge base inside the scene | No extra files | Doubles `show.json` for every applied scene |
| A pack id in two path segments (`/library/packs/{author}/{name}`) | No percent-encoding | Every route grows a parameter, and references (`author/pack:item`) still need encoding |
| Push packs to sources by provenance now | Matches the content page's wording | Sends data no source reads; v1 scenes are complete in `config.apply` |
| Categories, bands and colour ranges all in the UI | No manifest change | The menu's grouping of content would live outside the content, so a pack couldn't add to it |
| Backup including `secrets.json` | A restore carries pairing over to new hardware | A backup file becomes a credential; the identity keys must never leave the node ([security](../architecture/security.md)) |
| Restore merging with what is there | Keeps local additions | No clear answer for conflicts; a backup should give back exactly what it saved |

## Consequences

- **Contracts, additive:**
  - **Configuration** (`schema_version` stays 2): `snap` on `relative` regions; `hash` in `Provenance`, whose `pack` grows to 65 characters; `bindings` in `show.json`; `default_scene` up to 160 characters; `description` and `hints` on every pack item; `categories` in the manifest. The new fields have no default and are left out when absent, so existing documents, the migration's output and the clamp corpus's existing entries don't change; the corpus gains entries for them, appended.
  - **Client API** 1.4.0: the routes of D28 and their schemas (`ItemType`, `LibraryItem`, `ApplyResult`, `SceneApplied`, `ShowResetRequest`, `BackupManifest`), `keep_changes` in `SceneApplyRequest`, `editable` and `hash` in `PackInfo`, path parameters long enough for references, and the live channel's `event` ids up to 130 characters, for `event {kind: library}` with a pack id. The node routes the new paths to `501` until RB-09's changes land.
  - **Source link:** none; `packs.install` and `packs.remove` keep their shapes.
- **Documents:** [pack format](../specs/pack-format.md) (Accepted), [content](../architecture/content.md), [client API](../specs/client-api.md), [configuration model](../specs/config-model.md), [pipeline](../architecture/pipeline.md) (`snap`), [source link](../specs/source-link.md) (distribution), [effect specifications](../specs/effects/README.md) (new), [UI parity checklist](../specs/ui-parity-checklist.md) (where the content goes), [workflow](../process/workflow.md) and [M8](../runbooks/m8-release.md) (the gate in the release workflow), [M3](../runbooks/m3-controller-core.md) (RB-09's changes).
- **Code follows** in the changes listed in [RB-09](../runbooks/m3-controller-core.md#rb-09-content-library). Applying needs RB-08's zones only for their surfaces, which the documents already hold.
- **Reset all without a default** moves a boundary by one LED at lengths that aren't a multiple of 3 (D20).
- **The first public binary** waits on 24 specifications and rewrites (D24), which change the goldens of the six moving modes and of the scenes that use them.
- **Not covered by CI:** installs of packs made by other tools than FOLO's export, restore across node versions (only one format exists), and the UI's gallery, which arrives with RB-11.
