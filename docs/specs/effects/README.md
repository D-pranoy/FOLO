# Effect specifications

Status: Accepted · Implementation: Partial (the gate test `tests/test_clean_room_gate.py`; no specification is written yet)

Behaviour specifications of effects and of the scenes built on them, written so that an implementation can be made from them alone. The first ones are those the clean-room rewrite needs ([ADR-0023](../../decisions/ADR-0023-legacy-code-and-licensing.md) D3, [ADR-0038](../../decisions/ADR-0038-content-library.md) D23, D24).

## The clean-room rewrite gate

The six moving modes and the 18 LedFx-inspired effects were ported with reference to LedFx's source, which is GPL-3.0. Each is rewritten before the first public binary release:

1. **Specified:** someone who hasn't read LedFx's code or FOLO's port of it writes `docs/specs/effects/<id>.md` from observation (LedFx running, or FOLO's output) and LedFx's documentation. The specification says what the item draws, from which inputs, with which parameters and edge cases, precisely enough for golden tests. Its pull request names the writer's role (not their name) and states that rule was kept.
2. **Rewritten:** someone given only the specification (a person, or an agent session) reimplements the item. They work in a checkout without the item's current code (the mode code of `source/folo_source/effects/visualizers.py`, `web/`, and the scene in `packs/folo/effects/`), as a sparse checkout or a branch where it is removed, with no web access. Its pull request says so, and a maintainer reviews it for structure that follows LedFx rather than the specification (a model's training may have seen LedFx's public code). The goldens of what changed are regenerated, a behaviour change a maintainer approves.

The descriptions of the rewritten content keep "LedFx-inspired".

**The gate is a test.** `tests/test_clean_room_gate.py` checks that the table below names exactly the scenes tagged `LedFx` in the built-in packs and the engine's moving modes (`MOTION_MODES`), and that every row past `ported` has its specification file. With `FOLO_RELEASE_GATE=1`, which every publishing workflow sets (`release.yml` and `docker.yml`, `edge` images included), it fails unless every row is `rewritten`, so no public binary is published before that.

**Statuses:** `ported` (as today), `specified` (the specification is merged) and `rewritten` (reimplemented from it).

| Id | Item | Where | Specification | Status |
|---|---|---|---|---|
| `mode-scroll` | The `scroll` mode of the audio visualizer | `source/folo_source/effects/visualizers.py` | `docs/specs/effects/mode-scroll.md` | ported |
| `mode-drops` | The `drops` mode of the audio visualizer | `source/folo_source/effects/visualizers.py` | `docs/specs/effects/mode-drops.md` | ported |
| `mode-scan` | The `scan` mode of the audio visualizer | `source/folo_source/effects/visualizers.py` | `docs/specs/effects/mode-scan.md` | ported |
| `mode-wipe` | The `wipe` mode of the audio visualizer | `source/folo_source/effects/visualizers.py` | `docs/specs/effects/mode-wipe.md` | ported |
| `mode-flow` | The `flow` mode of the audio visualizer | `source/folo_source/effects/visualizers.py` | `docs/specs/effects/mode-flow.md` | ported |
| `mode-fire` | The `fire` mode of the audio visualizer | `source/folo_source/effects/visualizers.py` | `docs/specs/effects/mode-fire.md` | ported |
| `ledfx-energy` | The Energy scene | `folo/effects:ledfx-energy` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-energy.md` | ported |
| `ledfx-scroll` | The Scroll scene | `folo/effects:ledfx-scroll` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-scroll.md` | ported |
| `ledfx-wavelength` | The Wavelength scene | `folo/effects:ledfx-wavelength` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-wavelength.md` | ported |
| `ledfx-power` | The Power scene | `folo/effects:ledfx-power` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-power.md` | ported |
| `ledfx-rain` | The Rain scene | `folo/effects:ledfx-rain` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-rain.md` | ported |
| `ledfx-spotlight` | The Spotlight scene | `folo/effects:ledfx-spotlight` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-spotlight.md` | ported |
| `ledfx-scan` | The Scan scene | `folo/effects:ledfx-scan` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-scan.md` | ported |
| `ledfx-scan-multi` | The Scan multi scene | `folo/effects:ledfx-scan-multi` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-scan-multi.md` | ported |
| `ledfx-strobe` | The Strobe scene | `folo/effects:ledfx-strobe` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-strobe.md` | ported |
| `ledfx-blade-power` | The Blade Power+ scene | `folo/effects:ledfx-blade-power` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-blade-power.md` | ported |
| `ledfx-bar` | The Bar scene | `folo/effects:ledfx-bar` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-bar.md` | ported |
| `ledfx-multicolor-bar` | The Multicolor bar scene | `folo/effects:ledfx-multicolor-bar` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-multicolor-bar.md` | ported |
| `ledfx-bands` | The Bands scene | `folo/effects:ledfx-bands` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-bands.md` | ported |
| `ledfx-equalizer` | The Equalizer scene | `folo/effects:ledfx-equalizer` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-equalizer.md` | ported |
| `ledfx-blocks` | The Blocks scene | `folo/effects:ledfx-blocks` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-blocks.md` | ported |
| `ledfx-fire` | The Fire scene | `folo/effects:ledfx-fire` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-fire.md` | ported |
| `ledfx-lava-lamp` | The Lava lamp scene | `folo/effects:ledfx-lava-lamp` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-lava-lamp.md` | ported |
| `ledfx-marching` | The Marching scene | `folo/effects:ledfx-marching` (today `LEDFX_EFFECTS` in `web/studio.js`) | `docs/specs/effects/ledfx-marching.md` | ported |
