# ADR-0026: Branding and identifiers

Status: Accepted
Date: 2026-10-13

## Decision

| Item | Value |
|---|---|
| Product name | FOLO ("Fear of lights out") |
| Application id (Tauri, desktop entries, portal) | `io.github.d-pranoy.folo` |
| Publisher shown in installers | FOLO contributors |
| Primary contributors | Pranoy Das, Sayak Brahmachari |
| Windows service name | `FOLO` |
| Binaries | `folo` (node), `folo-source` (effect engine), `folo-capture` (capture agent), `FOLO` (desktop app) |
| Container image | `ghcr.io/d-pranoy/folo` |
| mDNS service | `_folo._tcp` |
| URI scheme | `folo://` (`join`, `connect`) |
| Environment prefix | `FOLO_` |
| Default port | 8777 |
| Arch package | `folo-bin` |

- Icons are placeholders until final artwork exists.
- The primary contributors are listed in the README, the About screen and package metadata.

## Consequences

These identifiers are stable. Changing one requires a superseding ADR and a migration plan.
