# Merge-train follow-ups

Status: Open

The findings from the v1 merge train, with the file, severity and suggested fix for each. The train stopped when #59 landed. The held capture PRs (#48, #49 and #54) landed afterwards, once their integration gaps were corrected (see the resolved table), and the licensing follow-ups keep the clean-room rewrite and release gates.

## High

| PR | File | Severity | Suggested fix |
|---|---|---|---|
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
| #56 | crates/folo-library/tests/structure.rs (`refused`) | Medium | Flaky on Windows. Every refusal test installs into the same `scratch("structure")` folder, and the tests run in parallel, so one test's `remove_dir_all` and another's `create_dir_all` collide ("Access is denied"); #48's first CI run failed `contents_lists_exactly_the_ids_of_the_files` that way, and the rerun passed. Give each test a folder of its own. |
| v1 | tests/test_screen_engine.py (`test_audio_steps_carry_the_newest_frame_while_audio_plays`) | Medium | Flaky on Windows. The test measures a 60 ms wall-clock window, and an extra idle render inside it fails `assert len(renders) == 1`; #59's first CI run saw `2 == 1`, and the rerun passed. Count the steps, or drive the clock, so the timer resolution does not decide the result. |

## Low

| PR | File | Severity | Suggested fix |
|---|---|---|---|
| #45 | docs/specs/source-link.md:3 | Low | Keep the Screen decoder progress and its exact validation and log-throttle summary from #40 when adding the capture-link status. |
| #51 | crates/folo-node/src/defaults.rs (tidy comment) | Low | Correct "Millimetre precision" to micrometre precision: rounding metres to 1e-6 is one micrometre. |
| #55 | AGENTS.md (Tests bullet) | Low | Remove the stray comma in `(also FOLO_E2E=1,; the test builds ...)`. |
| #57 | crates/folo-node/src/server.rs (`later_subsystems_answer_501_with_their_run_book`) | Low | The test pins `GET /api/v1/updates` (RB-17), since applying and backup are now served. Repoint it when RB-17 lands. |
| v1 | tests/test_mpris.py (`test_no_session_bus`) | Low | The test fails on a machine with a session bus, because it expects none (`DBUS_SESSION_BUS_ADDRESS` set). Clear the variable inside the test rather than relying on the runner's environment. Found in the #57 checks. |

## Resolved during the train

| PR | File | Resolution |
|---|---|---|
| #56 | crates/folo-library/src/canonical.rs (`number_text`) | Shortest-decimal tie handling now matches ECMAScript and Python, with shared tie vectors. Resolved in #56 (a0977ef). |
| #48 | crates/folo-capture/src/worker.rs and cli.rs | The worker reads `CaptureBackend::status()`: while the backend is paused the state is `paused`, the repeats carry `held`, and the status carries its `message` and `settings_uri`. Frames are stamped with `BackendChoice::clock()` (`QpcClock`). Scripted-backend and clock tests. Resolved in #48 (3da7629). |
| #49 | crates/folo-capture/src/worker.rs and backends.rs; `CaptureBackend::report()` | The worker takes the reported state (`waiting_consent`; an `error` ends the capture) and message, and keeps a handed-over restore token in `capture.status` until the capture ends; the portal stamps frames with `MonotonicClock`. Fake-report and clock tests. Resolved in #49 (dd3efef). |
| #54 | Linux capture-agent launch integration | Landed after #49; #55's replay end-to-end test, which the branch predated, now gives `CaptureCommand` its new `scope_launcher` (`None`). Resolved in #54 (c760721). |
| #45 | docs/runbooks/m6-screen.md:3 | The status line names the bar detector, the Screen effect and both backends again. Resolved in #48 (3da7629). |
