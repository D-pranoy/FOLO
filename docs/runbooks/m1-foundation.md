# M1 Foundation

Status: Accepted · Implementation: Not started

## RB-00: Governance and documentation

**Goal:** the documentation set, governance and repository hygiene are in place.
**Depends on:** nothing.

1. Merge `docs/` (this documentation set). Point `AGENTS.md` and `README.md` at it ([ADR-0001](../decisions/ADR-0001-documentation-is-the-source-of-truth.md)).
2. Add `CONTRIBUTING.md` (a summary of the [workflow](../process/workflow.md) and [testing policy](../process/testing-policy.md)), `SECURITY.md` (reporting and threat-model summary), `CODEOWNERS`, and PR and issue templates. The PR template has a "documentation impact" section.
3. Add `.editorconfig` and `.gitattributes` (LF; `*.bat eol=crlf`). Extend `.gitignore` with `dist/`, `build/`, `target/`, `node_modules/`, `data/` and `*.folopack` test outputs.
4. Apply the [repository settings checklist](../process/governance.md#repository-settings-checklist) (repository owner).
5. Tag the pre-rebuild state `v0.9.0-legacy` (maintainer approval) ([ADR-0023](../decisions/ADR-0023-legacy-code-and-licensing.md)).

**Done when:** docs are merged, branch protection is active, and templates are in place.

## RB-01: Toolchains and safety net

**Goal:** reproducible environments, linting, type checking, tests, golden frames that pin today's behaviour, and early packaging tests.
**Depends on:** RB-00.

1. **Python:**
   - `pyproject.toml` with hatchling and hatch-vcs; runtime numpy and soundcard; dev group pytest, hypothesis, pytest-cov, ruff, pyright, pre-commit.
   - Run `uv lock`.
   - Ruff rule sets E, F, I, UP, B, SIM, RUF, line length 100. Pyright basic across the repo, strict for new modules.
2. **Rust:**
   - A Cargo workspace (`crates/`) with `rustfmt.toml`, clippy pedantic for new crates, and `deny.toml`. Toolchain pinned in `rust-toolchain.toml`.
3. **UI:** reserve `ui/` (scaffolded in RB-11) and `shell/` (scaffolded in RB-19).
4. **pre-commit:** ruff, ruff-format, rustfmt, whitespace, JSON/YAML/TOML checks, codespell.
5. **Deterministic renderer:** let `Renderer` accept an injected random generator. Default behaviour must not change.
6. **Golden frames:** `scripts/make_golden.py` drives the current `Analyzer` and `Renderer` with deterministic synthetic stereo signals in 480-frame packets, and stores frames under `tests/golden/`.
   - Signals: sweeps, kicks at 120 BPM, pink noise, silence, out-of-phase stereo.
   - Configs: every mode × direction × trigger, with floor and peak variants, plus every built-in effect at three LED counts.
7. **Built-in export:** `scripts/export_builtins.mjs` evaluates the current UI's builder functions in Node (DOM stubs) for given LED counts. Its output feeds step 6 and RB-09.
8. **Pinning tests:** `tests/test_golden_renderer.py` asserts byte equality. Hypothesis tests for `sanitize_config` check that it never raises, is idempotent and respects limits, and record an input/output corpus for later ports.
9. **CI v0** (`ci.yml`):
   - Python lint, types and tests on Windows and Ubuntu 22.04 (install `libpulse0`)
   - Rust fmt, clippy and tests
   - coverage artifact
10. **Packaging tests:**
    - A PyInstaller one-dir build of the current studio (`--collect-data soundcard`) on Windows and Linux, smoke-tested with `--help`.
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
