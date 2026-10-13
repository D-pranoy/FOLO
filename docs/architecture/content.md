# Content: palettes, reactions, looks, scenes and packs

Status: Accepted · Implementation: Not started

Decided in [ADR-0021](../decisions/ADR-0021-content-packs.md). Formats are in [pack format](../specs/pack-format.md).

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
- **Applying a scene** matches roles to the zone's surfaces automatically, by capability, size and tags. The user can change each match, and the choice is remembered per scene and zone.
- **Fallbacks:**
  - A strip-role scene in a zone without a strip runs on `canvas.room` along `linear_x`.
  - Roles with no match are skipped, with a note.

## Simplicity and control

- **Simple:** a gallery with animated previews; one click applies a scene; 2–5 macro knobs per scene (intensity, speed, sensitivity, colours).
- **Mix and match:** swap the palette, reaction or effect of one role or layer.
- **Full control:** every parameter of every layer, through the schema-driven inspector.
- **No data is lost moving between levels.** Editing a parameter that a macro drives detaches that parameter only, and the UI marks it as overridden.

**Applying copies and records provenance** (pack, version, item id). When a pack updates, the UI offers "re-apply, keeping my changes".

## Built-in library

- Today's built-ins are JavaScript code in the current UI: 12 effects, 18 LedFx-inspired effects, 14 genre presets, 20 reactive presets and 9 palettes.
- They become data packs shipped with the node: `folo-core` (palettes, reactions), `folo-genres` and `folo-effects`.
- Stable ids replace links by name. An alias table keeps existing `default_preset` values (`fx:*`, `genre:*`) working.
- **Reset all** moves to the controller, so it works from any client and without a UI.
- The LedFx-inspired content is rewritten clean-room before the first public binary release ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)).

## Distribution

- The controller holds the library.
- Effects run in sources, so the controller works out which packs each source needs from its assigned scenes and pushes them over the source link, checked by hash.

## Later

- Recipe and expression effects: layered built-in building blocks with safe, vectorised expressions.
- A community pack index: a signed index in a Git repository, contributions by pull request, previews rendered in CI.
- Sandboxed WASM effect plugins.

See the [roadmap](../roadmap.md).
