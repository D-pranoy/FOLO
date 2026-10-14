# Workflow

Status: Accepted

## Documentation first

FOLO's documents are the source of truth ([ADR-0001](../decisions/ADR-0001-documentation-is-the-source-of-truth.md)). For any change:

1. **Find the owning document**: decision, specification, architecture, integration or run book.
2. **Change the document first,** in the same pull request as the code or an earlier one:
   - A change of decision needs a new ADR that supersedes the old one.
   - A breaking contract change needs an ADR and a contract version bump.
3. **Implement** to the document. Code that contradicts a document is a defect, whichever came first.
4. **Update** the document's `Implementation:` status when a milestone lands.

## Branches and commits

- **During the rebuild** ([ADR-0030](../decisions/ADR-0030-rebuild-on-a-v1-branch.md)):
  - Pull requests target `v1`, the default branch.
  - `main` is the frozen legacy studio: no merges and no fixes.
  - Both branches are protected: pull requests only, required checks, linear history ([governance](governance.md)).
- Branch names: `<type>/<short-topic>`, for example `feat/wled-ddp` or `docs/adr-0028`.
- **Conventional Commits** (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`, `chore:`; `!` for breaking changes). release-please builds the changelog and version from them.
- Squash-merge pull requests; the PR title becomes the commit message (the message body stays blank).

## Switch-over

When the roadmap says the rebuild replaces the studio ([roadmap](../roadmap.md#ordering-rules)), a maintainer:

1. Confirms the exit criteria of that milestone and that CI is green on `v1`.
2. Fast-forwards `main` to `v1`. This is possible because `main` took no commits ([ADR-0030](../decisions/ADR-0030-rebuild-on-a-v1-branch.md)). The ruleset has no bypass, and a pull request can't fast-forward under squash merging, so the repository owner switches the `main` ruleset off for the push and back on straight after.
3. Makes `main` the default branch again (repository owner) and keeps both rulesets matching.
4. Retargets open pull requests to `main`, then deletes `v1` or keeps it, as the maintainers decide.
5. Updates the branch references in the contributor guides and this document.

## Pull request checklist

- [ ] The relevant run book step is referenced (`RB-NN step N`) or the reason for the change is stated.
- [ ] **Documentation impact** is handled: docs updated, ADR added, or "none" explained.
- [ ] Tests are added or updated; golden files are regenerated only with a stated reason.
- [ ] Contracts and generated code are in sync (CI enforces this).
- [ ] No secrets, personal data or device addresses in code, tests or docs.
- [ ] Device-facing changes followed the [testing policy](testing-policy.md).

## CI gates (required to merge)

- Python: ruff, pyright, pytest (Windows and Linux)
- Rust: fmt, clippy, tests, cargo-deny
- UI: lint, type check, vitest, build
- End-to-end Playwright suite (once RB-11 lands)
- Contract validation and drift checks
- Dependency review
- CodeQL, from GitHub's default setup (never a workflow job, which conflicts with it); it can still be a required check

## Release checklist

1. The release PR from release-please is reviewed; the changelog is accurate.
2. For the **first public binary release**, the clean-room rewrite gate ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)) must be closed.
3. Merging the release PR tags the release; `release.yml` builds a draft release.
4. A maintainer verifies the artifacts (checksums, attestations, an install smoke test) and approves the `release` environment, which publishes the release.
5. Post-release:
   - publish the AUR `folo-bin` update and the Python client library (when changed)
   - update documentation statuses
   - announce
