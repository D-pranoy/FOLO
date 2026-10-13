# Run books

Status: Accepted

Run books break the [roadmap](../roadmap.md) into step-by-step work. Each run book is delivered as one or more pull requests that follow the [workflow](../process/workflow.md). A run book is done when its **Done when** criteria hold and CI is green.

## Conventions

- **Steps are ordered.** A step may be split across PRs, but not reordered past its dependencies.
- **Docs first.** If a step reveals that a decision or specification is wrong, stop and fix the document first, with an ADR if a decision changes.
- **Fakes before devices.** Device-facing steps build and test against fakes; live verification follows the [testing policy](../process/testing-policy.md).
- **Nothing outside the repository** (tags, releases, GitHub settings, devices) changes without a maintainer's approval ([ADR-0027](../decisions/ADR-0027-governance-and-repository-settings.md)).
- **No durations.** Run books never contain time estimates.

## Index

| Milestone | Run books |
|---|---|
| [M1 Foundation](m1-foundation.md) | RB-00 Governance and documentation · RB-01 Toolchains and safety net · RB-02 Contracts |
| [M2 Engine](m2-engine.md) | RB-03 Effect engine restructure · RB-04 Source pipeline and parity · RB-05 v1 configuration migration |
| [M3 Controller core](m3-controller-core.md) | RB-06 Node core · RB-07 Controller API · RB-08 Zones, projection and adapters · RB-09 Content library · RB-10 Hyperion provider |
| [M4 Interface](m4-interface.md) | RB-11 User interface |
| [M5 Devices](m5-devices.md) | RB-12 WLED provider · RB-13 Hue provider and zone sync |
| [M6 Screen](m6-screen.md) | RB-14 Display capture and Screen effect |
| [M7 Network](m7-network.md) | RB-15 Client access · RB-16 Source pairing and link · RB-17 OTA updates · RB-18 Home Assistant readiness |
| [M8 Release](m8-release.md) | RB-19 Desktop app · RB-20 Windows service and installer · RB-21 Linux packaging · RB-22 Docker image · RB-23 Release pipeline · RB-24 Documentation completion |
