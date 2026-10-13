# ADR-0019: CI/CD, versioning, releases and supply chain

Status: Accepted
Date: 2026-10-13

## Context

Releases must be reproducible, verifiable and safe to update to. The repository is public, which makes provenance attestations and CodeQL available and SignPath Foundation signing possible.

## Decision

- **D1.** **Workflows:**
  - `ci.yml`: Python, Rust and UI checks on Windows and Linux; end-to-end tests; contract checks; CodeQL; dependency review.
  - `build.yml`: reusable; every artifact.
  - `release.yml`: on tags.
  - `docker.yml`
  - `release-please.yml`
  - Nightly artifacts from `main` are kept 14 days and never published.
- **D2.** **Versioning:**
  - SemVer, with betas as `-beta.N`.
  - **Conventional Commits** and **release-please** keep versions in sync across `package.json`, `Cargo.toml` and `tauri.conf.json`, and maintain the changelog.
  - Python versions come from git tags.
- **D3.** **Releases** start as drafts. A protected `release` environment requires a maintainer's approval before publishing.
- **D4.** **Every release** includes `SHA256SUMS`, update signatures and manifests ([ADR-0020](ADR-0020-ota-updates.md)), and **build-provenance attestations**. Docker images get provenance and an SBOM.
- **D5.** **Supply chain:**
  - actions pinned by SHA
  - least-privilege `permissions:`
  - Dependabot for uv, npm, cargo, Actions and Docker
  - CodeQL
  - cargo-deny and cargo-audit
  - no secrets for pull requests from forks
- **D6.** **Windows code signing:** v1 ships unsigned, with checksums and attestations. Apply to SignPath Foundation (free for open source). The `signCommand` hook is reserved.
- **D7.** Images are published to GHCR.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Manual tags and changelog | Simple | Error-prone version sync across languages |
| semantic-release | Fully automatic | Publishes on every merge |
| Azure Artifact Signing now | Cheap and automated | Paid; individual eligibility limited by country |

## Consequences

Contributors follow Conventional Commits ([workflow](../process/workflow.md)).
