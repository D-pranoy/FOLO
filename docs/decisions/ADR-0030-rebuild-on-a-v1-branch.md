# ADR-0030: The rebuild lands on a `v1` branch

Status: Accepted
Date: 2026-10-14

## Context

- The rebuild replaces most of the code over several milestones ([roadmap](../roadmap.md)). For most of that time the new code can't do what the current studio does.
- `main` holds the working studio that people use today, and the roadmap keeps it usable until M3 replaces its server and M4 its UI.
- [ADR-0027](ADR-0027-governance-and-repository-settings.md), [ADR-0023](ADR-0023-legacy-code-and-licensing.md), [ADR-0019](ADR-0019-ci-cd-versioning-and-releases.md) and the [workflow](../process/workflow.md) assumed that work lands on `main`.
- Several GitHub features read only the default branch: issue templates, `SECURITY.md`, CodeQL default setup, Dependabot and release-please.
- The last commit before any documentation is `9c17033`. It is tagged `v0.9.0-legacy`.

## Decision

- **D1.** **Rebuild pull requests target `v1`.** The branch was cut from `main` at `9c17033`, the commit tagged `v0.9.0-legacy`.
- **D2.** **`main` is frozen** as the legacy studio: no merges and no fixes. The switch-over is then a fast-forward of `main` to `v1`.
- **D3.** **`v1` is the repository's default branch during the rebuild**, so the features that read only the default branch work from the start.
- **D4.** **Both branches are protected** by rulesets with the same rules ([governance](../process/governance.md#repository-settings-checklist)).
- **D5.** **The switch-over** is a maintainer step, listed in the [workflow](../process/workflow.md#switch-over) and the [roadmap](../roadmap.md#ordering-rules). It happens when the roadmap says the rebuild replaces the studio. After it, `main` is the default branch again, pull requests target `main`, and `v1` is deleted or kept as the maintainers decide.
- **D6.** **Amended texts.** ADR-0027 D2 and ADR-0023 D1 named `main`. They are amended in place, because neither was on a published branch yet, and both now cite this ADR. ADR-0019 D1 (nightly builds) follows the default branch.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Work on `main` directly | One branch; no switch-over | `main` stops being the working studio for most of the rebuild; fixes to the old studio mix with the new code |
| A long-lived branch per milestone | Small, focused branches | Many merges and conflicts; the default branch still holds code that is being replaced |
| A separate repository | Clean history for the new code | Loses stars, issues and history; links and identifiers change; contributors follow two repositories |

## Consequences

- The legacy studio gets no fixes; [`SECURITY.md`](../../SECURITY.md) says so.
- Documents that said "`main`" for where work lands, what is protected and what is built now say `v1` or "the default branch".
- Local clones and the contributor guides use `v1` as the base branch until the switch-over.
- Tools that follow the default branch (nightly builds, the Docker `edge` tag, CodeQL, Dependabot and release-please) need no change at the switch-over, apart from GitHub's default branch setting.
