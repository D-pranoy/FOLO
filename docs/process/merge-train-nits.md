# Merge-train follow-ups

Status: Open

The findings from the v1 merge train, with the file, severity and suggested fix for each. The train stopped when #59 landed. The held PRs stay unmerged until their integration gaps are corrected and reviewed, and the licensing follow-ups keep the clean-room rewrite and release gates.

## High

| PR | File | Severity | Suggested fix |
|---|---|---|---|
| #48 (held) | crates/folo-capture/src/worker.rs and cli.rs; backend status and clock APIs | HIGH | Integrate the backend's pause and privacy status and the selected OS clock into the worker, with fake-backend and clock tests. Without them, repeats lack `held`, stay `capturing` and omit the privacy URI (ADR-0037 D6, D12, D13). |
| #49 (held) | crates/folo-capture/src/worker.rs and cli.rs; `CaptureBackend::report()` | HIGH | Integrate the backend's report state and one-shot restore tokens into `capture.status`, and use the Linux monotonic frame clock, with fake-report, consent and token tests (ADR-0037 D3, D6, D14). A substantive review blocker. |
| #54 (held) | Linux capture-agent launch integration | HIGH | Depends on #49's corrected backend and worker integration. Land that first; this PR is held with #49. |
| #47 | docs/specs/effects/README.md (clean-room rewrite gate); ADR-0038 D17, D23, D24 | HIGH | Write and review clean-room specifications and rewrites for the six moving modes and the 18 LedFx-derived scenes. Until each is rewritten and gated, its port ships under GPL-3.0, as ADR-0038 requires. Keep the release gates enforced. |
| #52 | scripts/build_packs.py; packs/folo/effects/folo-pack.json | HIGH | Once the clean-room gate clears for the effects ports, return folo/effects to MIT. Regenerate its manifest and update the licence assertions together. |

## Medium

| PR | File | Severity | Suggested fix |
|---|---|---|---|
| #59 | scripts/conformance.py (`EXPECTED_ERRORS`, the API gate) | Medium | `EXPECTED_ERRORS` is keyed by phase, so any future errored stateful case passes. Key it by phase and operation (today `POST /auth/requests`), and verify the stated reason, because schemathesis deletes its NDJSON report on exit. |
| #56 | crates/folo-library/src/pack.rs (`check_files`) | Medium | Reject a files-map entry for folo-pack.json itself, and add a structural-validation test. Every other file is hash-checked, but the extra-key check accepts the manifest key because the archive contains it, and canonicalization then drops it. ADR-0038 D4 requires exactly the other entries. |
| #53 | crates/folo-provider-hue/tests/dtls.rs (`a_stream_the_bridge_closed_is_noticed_when_sending`) | Medium | Make the fake bridge's close-notify test deterministic. A close can already be observed by the first send, so handle that result or coordinate the close before testing later sends. The first Linux CI run failed at that unwrap while the local suite passed. |
| #57 | crates/folo-node/src/apply_api.rs (`apply_request`, `reset_request`, `read_base`, `write_base`) | Medium | Move blocking base-file reads, synced writes and the shared show/base transaction to the blocking pool, keeping lock coverage and ordering. An apply or reset must not occupy a Tokio worker during filesystem I/O. |
| #55 | crates/folo-node/tests/replay_e2e.rs; docs/runbooks/m6-screen.md step 9 | Medium | Extend replay coverage through projection to target frames, and scan the test temporary area as well as the node home, as the run book lists. Keep the fixture synthetic. |
| v1 | tests/test_screen_engine.py (`test_audio_steps_carry_the_newest_frame_while_audio_plays`) | Medium | Flaky on Windows. The test measures a 60 ms wall-clock window, and an extra idle render inside it fails `assert len(renders) == 1`; #59's first CI run saw `2 == 1`, and the rerun passed. Count the steps, or drive the clock, so the timer resolution does not decide the result. |

## Low

| PR | File | Severity | Suggested fix |
|---|---|---|---|
| #45 | docs/runbooks/m6-screen.md:3 | Low | Reconcile the progress headers with the merged bar detector (#42) and Screen effect (#40); the capture-link branch regresses these status summaries. |
| #45 | docs/specs/source-link.md:3 | Low | Keep the Screen decoder progress and its exact validation and log-throttle summary from #40 when adding the capture-link status. |
| #51 | crates/folo-node/src/defaults.rs (tidy comment) | Low | Correct "Millimetre precision" to micrometre precision: rounding metres to 1e-6 is one micrometre. |
| #55 | AGENTS.md (Tests bullet) | Low | Remove the stray comma in `(also FOLO_E2E=1,; the test builds ...)`. |
| #57 | crates/folo-node/src/server.rs (`later_subsystems_answer_501_with_their_run_book`) | Low | The test pins `GET /api/v1/updates` (RB-17), since applying and backup are now served. Repoint it when RB-17 lands. |
| v1 | tests/test_mpris.py (`test_no_session_bus`) | Low | The test fails on a machine with a session bus, because it expects none (`DBUS_SESSION_BUS_ADDRESS` set). Clear the variable inside the test rather than relying on the runner's environment. Found in the #57 checks. |

## Resolved during the train

| PR | File | Resolution |
|---|---|---|
| #56 | crates/folo-library/src/canonical.rs (`number_text`) | Shortest-decimal tie handling now matches ECMAScript and Python, with shared tie vectors. Resolved in #56 (a0977ef). |
