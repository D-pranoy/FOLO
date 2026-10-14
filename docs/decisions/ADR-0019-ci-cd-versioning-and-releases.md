# ADR-0019: CI/CD, versioning, releases and supply chain

Status: Accepted
Date: 2026-10-13

## Context

Releases must be reproducible, verifiable and safe to update to. The repository is public, which makes provenance attestations and CodeQL available and SignPath Foundation signing possible.

## Decision

- **D1.** **Workflows:**
  - `ci.yml`: Python, Rust and UI checks on Windows and Linux; end-to-end tests; contract checks; dependency review. It runs on pushes and pull requests for `v1` and `main` ([ADR-0030](ADR-0030-rebuild-on-a-v1-branch.md)). CodeQL is not a job here: it runs from GitHub's default setup (D5).
  - `build.yml`: reusable; every artifact.
  - `release.yml`: on tags.
  - `docker.yml`
  - `release-please.yml`
  - Nightly artifacts from the default branch (`v1` during the rebuild) are kept 14 days and never published.
- **D2.** **Versioning:**
  - SemVer, with betas as `-beta.N`.
  - **Conventional Commits** and **release-please** keep versions in sync across `package.json`, `Cargo.toml` and `tauri.conf.json`, and maintain the changelog.
  - Python versions come from git tags. Only FOLO's release tags (`v<semver>`) count: the tag `v0.9.0-legacy` isn't a valid PEP 440 version, and local clones carry upstream tags such as `v1.1.0`.
  - release-please opens its pull requests and creates its tags with a **GitHub App installation token**, because pull requests and tags made with the default `GITHUB_TOKEN` don't start workflows: release pull requests would get no CI, and the tag wouldn't start `release.yml`. The App is created by the repository owner ([governance](../process/governance.md#key-custody)).
- **D3.** **Releases** start as drafts. A protected `release` environment requires a maintainer's approval before publishing.
- **D4.** **Every release** includes `SHA256SUMS`, update signatures and manifests ([ADR-0020](ADR-0020-ota-updates.md)), and **build-provenance attestations**. Docker images get provenance and an SBOM.
- **D5.** **Supply chain:**
  - only GitHub-authored actions and a named allowlist of third-party actions, all pinned to a full commit SHA ([governance](../process/governance.md#allowed-actions)); the repository setting that requires SHA pinning is on
  - least-privilege `permissions:`
  - Dependabot for uv, npm, cargo, Actions and Docker
  - CodeQL through GitHub's default setup only; a CodeQL job in a workflow conflicts with it
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
