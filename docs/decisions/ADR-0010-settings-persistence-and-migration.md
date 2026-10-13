# ADR-0010: Settings, persistence and migration

Status: Accepted
Date: 2026-10-13

## Context

- The node runs in four ways: managed by the desktop app, as a service, from the CLI, or in Docker.
- Settings must be predictable in each case, and editable from the UI unless fixed by the deployment.
- Today's studio stores everything in one `studio.json` next to the code.

## Decision

- **D1.** **Settings layering** follows Paseo's model:
  - Managed launches: defaults → config file, with inherited `FOLO_*` variables removed.
  - Foreground launches: defaults → config file → `FOLO_*` → CLI flags.
  - Overrides are shown locked in the UI.
  - List settings add up across sources.
- **D2.** Data lives in per-OS folders (platformdirs conventions):
  - user mode: `%LOCALAPPDATA%\FOLO` on Windows, XDG directories on Linux
  - service: `%ProgramData%\FOLO` on Windows, `/var/lib/folo` on Linux
  - portable: `.\data`
  - Docker: `/data`
- **D3.** Persistence is **versioned JSON documents**: zones, show, providers, sources, clients, library. Writes are atomic and debounced, with a revision and server id per document. Secrets are in a separate owner-only file ([config model](../specs/config-model.md)).
- **D4.** Validation **clamps and never rejects** content values, as today's `sanitize_config()` does.
- **D5.** Version-1 `studio.json` files are **imported automatically on first run** when found, with a backup kept. `folo config import` does the same on demand.
- **D6.** Operations: `folo status --json`, `folo reload` (hot-reload with restart-required reporting), and `/api/v1/health` without authentication.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| SQLite | Transactions | Opaque; migration tooling needed; little benefit at this size |
| One JSON file | As today | Large rewrites; secrets mixed with content |

## Consequences

The migration tool lives with the Python engine, which already understands v1 ([RB-05](../runbooks/m2-engine.md#rb-05-v1-configuration-migration)).
