# Content: palettes, reactions, looks, scenes and packs

Status: Accepted · Implementation: Not started

Decided in [ADR-0021](../decisions/ADR-0021-content-packs.md) and [ADR-0038](../decisions/ADR-0038-content-library.md). Formats are in [pack format](../specs/pack-format.md), and the library's routes in the [client API](../specs/client-api.md#library).

## Levels of content

| Level | Holds | Today's equivalent |
|---|---|---|
| Palette | Colours, optionally with white stops such as `2700K` | `PRESETS` in the current UI |
| Reaction | How a layer responds to audio: band, trigger, thresholds, gain, curve, attack, release | Reactive presets (`REACTIVE`, `REACT`) |
| Effect | A renderer with a parameter schema and minimum capabilities | The 10 modes |
| Look | One layer's full recipe: effect + parameters + reaction + palette | Reactive preset + palette |
| Scene | Layer stacks for a zone's surfaces, by role, with macro knobs | Effects, LedFx effects, genre presets, saved presets |
| Zone template | A target layout without addresses or keys (for example "TV backlight on 4 sides + 2 lamps") | New |
| Pack | A versioned bundle of any of the above | New |

Each level refers to the levels below it by id. A scene can swap only its palette, and a pack can contain only palettes.

## Portability across setups

- Scenes target **roles**, not LED indices: `strip.main`, `canvas.room`, `canvas.screen`, `points.accent`, and so on.
- Layers are placed at **relative positions** (0–1) along a strip or within a canvas. They use the layout generators from today's built-ins, expressed as data: alternate, zones, mirror, stack, per band.
- **Applying a scene** matches roles to the zone's surfaces automatically, by kind, capability and size. The user can change each match, and the choice is remembered per scene and zone.
- **Fallbacks:**
  - A strip-role scene in a zone without a strip runs on `canvas.room` along `linear_x`.
  - Roles with no match are skipped, with a note.

## Applying a scene

Decided in [ADR-0038](../decisions/ADR-0038-content-library.md) D11 to D16.

- **The copy** goes into the zone's active scene in place, when no other zone has that scene active, and otherwise into a new scene that becomes the zone's active one. It carries the item's name, tags and macros, its layers bound and scaled as below, and `provenance {pack, version, item, hash}`. The node keeps the copy as written in `applied/<scene id>.json`, the base for a later merge.
- **Binding,** for each of the scene's roles in order, each surface of the zone taken at most once:
  1. an override: the surface the request names, else the one remembered for this scene and zone
  2. the surface with the same role, first in `zones.json` order
  3. a surface of the same kind (`strip.*` roles a strip, `canvas.*` roles a canvas): the one whose cell count is nearest the scene's `led_count` when it has one, else the one with the most cells; ties in `zones.json` order. Other roles (`points.*`) match by steps 1 and 2 only.
  4. a surface qualifies in steps 2 and 3 only with at least as many cells as the role's effects need (`min_elements` in the catalog)
  5. the item's `fallbacks` for the role; then, for a `strip.*` role, `canvas.room`, else the zone's first canvas
  6. otherwise the role is skipped and the answer says so

  The copy lists a bound role's layers under the bound surface's role, which is how the engine finds them, so surfaces that share a role share layers.
- **Remembered overrides** are the show's `bindings`, per zone and scene. Naming a surface for a role remembers it; naming `""` returns the role to automatic.
- **Scaling:** `cells` regions of a scene with a `led_count` are scaled to the bound surface's width as the studio scales saved presets (`scaleSegments()`), halves rounding up. `relative` and `rect` regions need nothing.
- **Re-apply, keeping my changes:** a three-way merge of the scene now (mine), the base kept at the last apply, and the item as an apply would write it now (theirs). A value mine left as it was takes theirs; a value mine changed stays. Layers and macros are matched by id; a list is one value; layers the user added stay, layers they removed stay removed, and new layers of the item arrive.
- **Macros** are evaluated by clients, which write the parameters a knob drives. The node and the engine ignore them.

## Simplicity and control

- **Simple:** a gallery with animated previews; one click applies a scene; 2–5 macro knobs per scene (intensity, speed, sensitivity, colours).
- **Mix and match:** swap the palette, reaction or effect of one role or layer.
- **Full control:** every parameter of every layer, through the schema-driven inspector.
- **No data is lost moving between levels.** Editing a parameter that a macro drives detaches that parameter only, and the UI marks it as overridden.

**Applying copies and records provenance** (pack, version, item id). When a pack updates, the UI offers "re-apply, keeping my changes".

## Built-in library

- Today's built-ins are JavaScript code in the current UI:
  - 14 effects, including the beat-based Beat sync and Half-time pulse
  - 18 LedFx-inspired effects (Bar and Multicolor bar use the beat trigger)
  - 14 genre presets
  - 25 reactive presets in 5 groups, including "On the beat"
  - 9 palettes
- They become data packs embedded in the node ([ADR-0038](../decisions/ADR-0038-content-library.md) D17 to D20):

  | Pack | Holds |
  |---|---|
  | `folo/core` | 9 palettes, 25 reactions (the reactive presets, with their groups as hints), the scene `default` (today's reset segments) and the menu's 8 categories |
  | `folo/genres` | 14 scenes, one per genre preset |
  | `folo/effects` | 32 scenes: the 14 effects and the 18 LedFx-inspired effects |

- **Stable ids** replace links by name: `genre:edm` becomes `folo/genres:edm`, `fx:beat-sync` becomes `folo/effects:beat-sync`. The **alias table** (`packs/aliases.json`) keeps every existing `fx:*` and `genre:*` id working, in `default_scene` and when applying. A bare id names a scene of the show, else a saved preset in `local/my-presets`.
- **As data:** each built-in scene has one role, `strip.main`, and one `audio-visualizer` layer per segment its builder makes, placed by a `relative` region with `snap: nearest` at the builder's fractions, so it gives the builder's LEDs at every length. Until the UI's switch-over removes `web/`, `scripts/build_packs.py` generates the packs from the builders, and CI checks they are up to date and equivalent (the built-in golden frames byte for byte, and the layers at lengths from 1 to 2000).
- The seven bands and six colour ranges of the editors stay in the UI.
- **Reset all** moves to the controller (`POST /show/reset`), so it works from any client and without a UI: each zone gets the default scene, else `folo/core:default`, with master 1 and a black background.
- The LedFx-inspired content is rewritten clean-room before the first public binary release ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)), tracked in the [effect specifications](../specs/effects/README.md).

## Distribution

- The controller holds the library, and backs it up with the documents ([client API](../specs/client-api.md#backup-and-restore)).
- **In v1 no pack goes to a source:** an applied scene is a complete copy, and reaches its source whole in `config.apply`. When content that a source must hold arrives (recipe effects and plugins, below), the controller sends those packs over the source link, checked by their hash ([source link](../specs/source-link.md#content-distribution), [ADR-0038](../decisions/ADR-0038-content-library.md) D22).

## Later

- Recipe and expression effects: layered built-in building blocks with safe, vectorised expressions.
- A community pack index: a signed index in a Git repository, contributions by pull request, previews rendered in CI.
- Sandboxed WASM effect plugins.

See the [roadmap](../roadmap.md).
