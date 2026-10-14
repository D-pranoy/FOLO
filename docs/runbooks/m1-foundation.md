# M1 Foundation

Status: Accepted · Implementation: Partial (RB-00 steps 1–4)

## RB-00: Governance and documentation

**Goal:** the documentation set, governance and repository hygiene are in place.
**Depends on:** nothing.

1. **Branch and tag** (maintainer) ([ADR-0030](../decisions/ADR-0030-rebuild-on-a-v1-branch.md), [ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)):
   - Tag `9c17033`, the last legacy commit before any documentation, as `v0.9.0-legacy` (annotated), and create `v1` from it.
   - Push the branch and that single tag: `git push origin v1 v0.9.0-legacy`. Never `git push --tags`: local clones carry upstream tags such as `v1.1.0`.
   - From here on, rebuild work lands on `v1` and `main` is frozen.
2. Merge `docs/` (this documentation set) into `v1`. Point `AGENTS.md` and `README.md` at it ([ADR-0001](../decisions/ADR-0001-documentation-is-the-source-of-truth.md)).
3. Add `CONTRIBUTING.md` (a summary of the [workflow](../process/workflow.md) and [testing policy](../process/testing-policy.md)), `SECURITY.md` (reporting and threat-model summary), `CODEOWNERS`, and PR and issue templates. The PR template has a "documentation impact" section.
4. Add `.editorconfig` and `.gitattributes` (LF; `*.bat eol=crlf`). Extend `.gitignore` with `dist/`, `build/`, `target/`, `node_modules/`, `/data/` and `*.folopack` test outputs.
5. Apply the [repository settings checklist](../process/governance.md#repository-settings-checklist) (repository owner): `v1` as the default branch, rulesets on `v1` and `main`, the Actions policy and the release App.
6. **Remove the legacy effect runner** in a pull request into `v1` ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)):
   - Delete `main.py`, `effects/`, `lib/`, `app/gui.py`, `app/hyperion.py`, `matrix-config.js`, `toggle.sh`, `.asoundrc`, `config.json` and `run.bat` (the list in [current system](../architecture/current-system.md#shape)).
   - Drop `soundcard` from `requirements.txt`. Only `effects/spectrum_dump.py` imports it (check with a search first). Drop any other requirement that only the removed code imports.
   - Update `AGENTS.md` (commands, the legacy effects section), `README.md` (the legacy effects and Linux setup sections) and [current system](../architecture/current-system.md).
   - The studio must still start and pass the syntax checks.

**Done when:** `v1` exists with the tag `v0.9.0-legacy` on its base, docs are merged, branch protection is active on `v1` and `main`, templates are in place, and the legacy runner is gone from `v1` with the studio still running.

## RB-01: Toolchains and safety net

**Goal:** reproducible environments, linting, type checking, tests, golden frames that pin today's behaviour, and early packaging tests.
**Depends on:** RB-00.

1. **Python:**
   - `pyproject.toml` with hatchling and hatch-vcs; runtime numpy; dev group pytest, hypothesis, pytest-cov, ruff, pyright, pre-commit.
   - **Version tags:** hatch-vcs uses only FOLO's release tags, through a `tag-pattern` (or `git_describe_command`) that matches `v<semver>` releases. It must exclude `v0.9.0-legacy`, which isn't a valid PEP 440 version, and upstream tags such as `v1.1.0` that local clones carry ([ADR-0019](../decisions/ADR-0019-ci-cd-versioning-and-releases.md)).
   - Run `uv lock`.
   - Ruff rule sets E, F, I, UP, B, SIM, RUF, line length 100. Pyright basic across the repo, strict for new modules.
2. **Rust:**
   - A Cargo workspace (`crates/`) with `rustfmt.toml`, clippy pedantic for new crates, and `deny.toml`. Toolchain pinned in `rust-toolchain.toml`.
3. **UI:** reserve `ui/` (scaffolded in RB-11) and `shell/` (scaffolded in RB-19).
4. **pre-commit:** ruff, ruff-format, rustfmt, whitespace, JSON/YAML/TOML checks, codespell.
5. **Deterministic renderer:** let `Renderer` accept an injected random generator. Default behaviour must not change.
6. **Golden frames:** `scripts/make_golden.py` drives the current `Analyzer` and `Renderer` with deterministic synthetic stereo signals in 480-frame packets, and stores frames under `tests/golden/`.
   - Signals: sweeps, kicks at 120 BPM, pink noise, silence, out-of-phase stereo.
   - Configs:
     - every mode × direction × trigger (level, hits, beat with every `beats` value), with floor and peak variants
     - gains up to +54 dB, auto level on and off
     - reactive Auto/Off with solid and breathing ambient light
     - every built-in effect at three LED counts
   - **Beat-tracker fixtures:** synthetic songs with known beat times, plus beatless music.
7. **Built-in export:** `scripts/export_builtins.mjs` evaluates the current UI's builder functions in Node (DOM stubs) for given LED counts. Its output feeds step 6 and RB-09.
8. **Pinning tests:** `tests/test_golden_renderer.py` asserts byte equality. Hypothesis tests for `sanitize_config` check that it never raises, is idempotent and respects limits, and record an input/output corpus for later ports.
9. **CI v0** (`ci.yml`), on pushes and pull requests for `v1` and `main`:
   - Python lint, types and tests on Windows and Ubuntu 22.04 (install `libpulse0`)
   - Rust fmt, clippy and tests
   - coverage artifact
   - Once it has run, a maintainer adds its jobs as required checks in the `v1` ruleset ([governance](../process/governance.md#repository-settings-checklist)).
   - CodeQL is not a job here; it comes from GitHub's default setup ([allowed actions](../process/governance.md#allowed-actions)).
10. **Packaging tests:**
    - A PyInstaller one-dir build of the current studio on Windows and Linux, smoke-tested with `--help`. It must include cffi and the WASAPI module's C definitions, and the `winrt` packages on Windows.
    - A `cargo build --release` of an empty `folo` binary on every target runner.

**Done when:** CI is green on Windows and Linux, golden frames and the corpus are committed, and the frozen builds start.

## RB-02: Contracts

**Goal:** machine-readable contracts and generated types for all three languages.
**Depends on:** RB-01.

1. Write JSON Schemas in `specs/schema/` for every entity in the [configuration model](../specs/config-model.md), including the reserved `display` target. Express clamping limits as `minimum`/`maximum` and defaults.
2. Write `specs/openapi.yaml` (OpenAPI 3.1) from the [client API](../specs/client-api.md), and `specs/asyncapi.yaml` (AsyncAPI 3.0) for the live channel, the source link, ingest and their binary frames.
3. **Code generation:**
   - Python: `datamodel-code-generator` → pydantic models in the effect engine.
   - Rust: `typify` → a `folo-protocol` crate.
   - TypeScript: `openapi-typescript` → `ui/src/lib/api`.
   - Generated code is committed and checked for drift in CI.
4. **Clamp-not-reject:** a shared test corpus checks that the Python and Rust clampers produce identical output for the same input, including the RB-01 `sanitize_config` corpus for v1-shaped data.
5. **Conformance harness:** a test runner that drives any implementation of the API or source link against the specs (schemathesis for OpenAPI; message fixtures for AsyncAPI).
6. **CI:** validate OpenAPI and AsyncAPI; fail on drift between prose specs, machine-readable contracts and generated code.

**Done when:** generated types build in all three languages and contract checks run in CI.
