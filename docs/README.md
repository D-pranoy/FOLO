# FOLO documentation

The documents in this folder are the **source of truth** for FOLO. The software exists to implement them. When the code and these documents disagree, the code is wrong, or the documents must be changed first through the process in [process/workflow.md](process/workflow.md).

FOLO is being rebuilt from a single-machine audio studio into a modular, spatial lighting-management system. These documents describe the **target system**. Until each milestone of the [roadmap](roadmap.md) is implemented, the repository still contains the earlier studio, described in [architecture/current-system.md](architecture/current-system.md) and in `AGENTS.md`.

## Order of authority

When documents conflict, the higher entry wins. Fix the lower one.

1. [Decisions](decisions/README.md): accepted architecture decision records (ADRs)
2. [Specifications](specs/README.md): contracts, formats and protocols
3. [Architecture](architecture/overview.md) and [integrations](integrations/): how the system is built
4. [Run books](runbooks/README.md): how each piece of work is carried out
5. Code, tests and inline comments

## Map

| Area | Documents |
|---|---|
| Why and what | [vision.md](vision.md) · [scope.md](scope.md) · [roadmap.md](roadmap.md) · [glossary.md](glossary.md) |
| Architecture | [overview](architecture/overview.md) · [pipeline](architecture/pipeline.md) · [spatial model](architecture/spatial-model.md) · [capabilities](architecture/capabilities.md) · [control plane](architecture/control-plane.md) · [security](architecture/security.md) · [updates](architecture/updates.md) · [content](architecture/content.md) · [user interface](architecture/ui.md) · [platform and packaging](architecture/platform-and-packaging.md) · [projection-mapping groundwork](architecture/projection-mapping-groundwork.md) · [current system](architecture/current-system.md) |
| Integrations | [Philips Hue](integrations/hue.md) · [WLED](integrations/wled.md) · [Hyperion](integrations/hyperion.md) · [display capture](integrations/display-capture.md) · [Home Assistant](integrations/home-assistant.md) |
| Specifications | [index](specs/README.md) · [configuration model](specs/config-model.md) · [client API](specs/client-api.md) · [source link](specs/source-link.md) · [pairing and connect](specs/pairing-and-connect.md) · [pack format](specs/pack-format.md) · [UI parity checklist](specs/ui-parity-checklist.md) · [visual design](specs/visual-design.md) |
| Decisions | [register and ADRs](decisions/README.md) |
| Run books | [index](runbooks/README.md) |
| Process | [workflow](process/workflow.md) · [testing policy](process/testing-policy.md) · [governance](process/governance.md) |

## Document conventions

- **Status header.** Every document starts with `Status:` (Draft, Accepted, Superseded). Specifications also carry `Implementation:` (Not started, Partial, Complete).
- **No time estimates.** Plans and the roadmap describe order and dependencies, never durations or dates.
- **No personal details.** Documents describe the project, not who owns which hardware or who will do which task.
- **Stable identifiers.** ADRs are numbered (`ADR-0007`), run books are numbered (`RB-12`), and requirements inside specifications use local IDs where useful. Never renumber; supersede instead.
- **Links over repetition.** State a fact once, in the document that owns it, and link to it elsewhere.
- **Sources.** Facts about third-party protocols and platforms cite their source.
