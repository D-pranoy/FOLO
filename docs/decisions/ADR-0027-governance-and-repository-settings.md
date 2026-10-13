# ADR-0027: Governance and repository settings

Status: Accepted
Date: 2026-10-13

## Context

The project has a repository owner, who holds GitHub administration, and a lead maintainer, who makes technical decisions with the owner's trust. Most work is done by one active maintainer with AI assistance.

## Decision

- **D1.** **Roles** are defined in [governance](../process/governance.md):
  - The **repository owner** administers GitHub settings, secrets and packages.
  - The **lead maintainer** decides technical matters and accepts ADRs.
- **D2.** **Branch protection on `main`:** pull requests only; required CI checks; linear history; no required approvals (one active maintainer). `CODEOWNERS` assigns the lead maintainer.
- **D3.** **Repository settings:**
  - public visibility
  - Actions token read-only by default
  - a `release` environment requiring the lead maintainer's approval
  - GHCR package visibility public
  - Dependabot alerts and security updates on
- **D4.** **Secrets:** the update signing key and its password are stored as Actions secrets. The lead maintainer generates them; the repository owner keeps an offline backup.
- **D5.** Changes visible outside the repository (tags, releases, settings, issues, package publishing) are made or requested by maintainers through this process, never by automation without approval.

## Consequences

The setting changes are listed as a checklist in [governance](../process/governance.md#repository-settings-checklist).
