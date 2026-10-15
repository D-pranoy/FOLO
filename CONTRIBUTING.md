# Contributing to FOLO

Thanks for helping. This page summarises how work gets into FOLO. The full rules live in [docs/process/workflow.md](docs/process/workflow.md) and [docs/process/testing-policy.md](docs/process/testing-policy.md); where this page and those documents differ, the documents win.

## Documentation first

The documents in [docs/](docs/README.md) are the **source of truth** for FOLO, and the software exists to implement them ([ADR-0001](docs/decisions/ADR-0001-documentation-is-the-source-of-truth.md)). Code that contradicts a document is a defect.

1. **Find the owning document:** a decision (ADR), specification, architecture or integration document, or run book. When documents conflict, ADRs beat specifications, which beat architecture, which beats run books, which beat code.
2. **Change the document first,** in the same pull request as the code or an earlier one. A changed decision needs a new ADR that supersedes the old one; a breaking contract change needs an ADR and a contract version bump.
3. **Implement** to the document.
4. **Update** the document's `Implementation:` status when a milestone lands.

FOLO is being rebuilt step by step. The [roadmap](docs/roadmap.md) orders the milestones, and the [run books](docs/runbooks/README.md) break each one into steps. The rebuild lands on the `v1` branch ([ADR-0030](docs/decisions/ADR-0030-rebuild-on-a-v1-branch.md)). `main` is the frozen earlier studio, described in [docs/architecture/current-system.md](docs/architecture/current-system.md).

**Writing documents:**
- Every document starts with a `Status:` line. Documents that describe something to build (specifications, architecture, integrations and run books) also carry `Implementation:` (Not started, Partial or Complete).
- No time estimates and no personal details.
- Never renumber ADRs or run books; supersede them.
- State a fact once and link to it elsewhere. Cite sources for third-party protocols and platforms.

For a bigger idea, open a feature request first, so it can be weighed against the [scope](docs/scope.md) before anyone writes code.

## Branches, commits and pull requests

- Pull requests target `v1`, the default branch during the rebuild. `main` is frozen and takes no pull requests ([ADR-0030](docs/decisions/ADR-0030-rebuild-on-a-v1-branch.md)). Both branches are protected: pull requests only, with linear history. Pull requests are squash-merged.
- Branch names: `<type>/<short-topic>`, for example `feat/wled-ddp` or `docs/adr-0030`.
- Commits and PR titles follow **Conventional Commits** (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`, `chore:`; `!` for a breaking change). The PR title becomes the commit message, and release-please builds the changelog and version from it.
- The pull request template asks for:
  - the run book step (`RB-NN step N`) or the reason for the change
  - the **documentation impact**: documents updated, an ADR added, or why there's none
  - tests added or updated, with a reason for any regenerated golden file
  - contracts and generated code in sync
  - no secrets, personal data or device addresses in code, tests or docs
- CI must pass before merging. The required gates are listed in the [workflow](docs/process/workflow.md#ci-gates-required-to-merge); they grow as the rebuild adds Python, Rust and UI toolchains.

## Setting up

You need [uv](https://docs.astral.sh/uv/) for Python 3.13 and the locked dependencies, and [rustup](https://rustup.rs/) for Rust (the version is pinned in `rust-toolchain.toml`). The current studio runs on Windows; its tests also run on Linux.

```
uv sync                          # Python 3.13 and the locked dependencies, in .venv
uv run pre-commit install        # run the hooks on every commit
uv run pytest                    # golden frames, beat fixtures, sanitize_config
uv run ruff check . && uv run pyright
cargo clippy --all-targets -- -D warnings && cargo test
uv run python studio.py          # the studio, on http://127.0.0.1:8777/
```

If you added RanzQ's repository as `upstream`, run this once. Its old tags (`v0.1`, `v1.0`, `v1.1.0`) would otherwise clash with FOLO's releases and versions ([ADR-0019](docs/decisions/ADR-0019-ci-cd-versioning-and-releases.md)):

```
git config remote.upstream.tagOpt --no-tags   # fetching upstream never brings its tags
git tag -d v0.1 v1.0 v1.1.0                    # remove the ones already fetched
```

[README.md](README.md) and [AGENTS.md](AGENTS.md) describe the commands, the architecture of the current code and how to test it. AI coding agents follow AGENTS.md.

## Testing

- **Layers:** unit, golden, contract and integration tests run on every pull request, and end-to-end UI tests once the new UI exists. Packaging tests run on release builds; live verification on real devices happens before milestones close and before releases. See [test layers](docs/process/testing-policy.md#test-layers).
- **Golden files** pin behaviour: the studio's rendered frames (PNGs in `tests/golden/renderer/`, time down and LEDs across, so they can be looked at), the beat tracker on synthetic songs, and `sanitize_config`'s outputs. Regenerate them with `uv run python scripts/make_golden.py` only when behaviour is meant to change, and say why in the pull request. Review image goldens visually.
- **No real audio or screens in the repository:** synthetic inputs and recorded fixtures are committed, recordings of real audio or screens never are.
- **Without sound or devices:** synthetic songs for the beat tracker, an inaudible tone for app capture, fakes for every device and a fake media session for player controls. See [testing without sound or devices](docs/process/testing-policy.md#testing-without-sound-or-devices).
- **Speed-ups must not change the output:** compare the old and new code on the same audio with seeded randomness, frame for frame.

## Real devices

FOLO drives real lights in people's homes. Device-facing work is built and tested against fakes first, and live tests follow the [live-device safety rules](docs/process/testing-policy.md#live-device-safety-rules):

1. **Never** test against someone's live configuration or devices without their consent. Use a separate configuration (or data folder) and a separate port.
2. **Announce** what will change on a device or bridge before doing it.
3. Start at **low brightness**, one target at a time, with the flash limiter on.
4. **Release** devices afterwards: stop streams, clear priorities, restore changed settings, and confirm the owner's configuration is untouched.
5. **Hue:** stop Hue Sync or any other streaming app first, and never take over another app's stream without the owner's confirmation.
6. Change bridges and controllers only while their owner can watch the result.

## Licensing and third-party code

- FOLO is MIT-licensed ([LICENSE](LICENSE)), and contributions are accepted under the same licence.
- Code copied from elsewhere must have a compatible licence and be recorded in `THIRD_PARTY_NOTICES`. Never copy diyHue's `Lights/` (CC BY-NC) or Signify's documentation.
- **LedFx is GPL-3.0.** Don't copy or translate its code. The LedFx-inspired modes and effects are rewritten clean-room from behaviour specifications ([ADR-0023](docs/decisions/ADR-0023-legacy-code-and-licensing.md)).

## Security

Report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md), never in a public issue.
