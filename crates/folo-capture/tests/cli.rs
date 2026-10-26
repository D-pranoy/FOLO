//! The `folo-capture` binary's command line: exit statuses (ADR-0037 D2) and
//! `screens --json` on the synthetic and replay backends.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use folo_capture_core::replay;
use folo_capture_core::types::{Bars, FrameFlags, Picture, ScreenFrame};
use serde_json::{Value, json};

const FOLO_CAPTURE: &str = env!("CARGO_BIN_EXE_folo-capture");
const SECRET_TOKEN: &str = "launch-token-that-must-not-be-printed";

/// A fresh folder, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "folo-capture-cli-test-{}-{number}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `folo-capture <args>` without the caller's `FOLO_*` variables, plus `env`.
fn agent(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(FOLO_CAPTURE);
    command.args(args);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("FOLO_") {
            command.env_remove(name);
        }
    }
    command.envs(env.iter().copied()).output().unwrap()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn replay_file(scratch: &Scratch) -> PathBuf {
    let frame = ScreenFrame {
        sequence: 0,
        timestamp_us: 0,
        picture: Picture {
            screen_index: 0,
            width: 16,
            height: 9,
            flags: FrameFlags::default(),
            bars: Bars::default(),
            pixels: vec![50; 16 * 9 * 3],
        },
    };
    scratch.write("screen.folr", &replay::write(&[frame]).unwrap())
}

fn linked() -> [(&'static str, &'static str); 2] {
    [
        (
            "FOLO_CAPTURE_LINK",
            "ws://127.0.0.1:8777/api/v1/capture-link",
        ),
        ("FOLO_CAPTURE_TOKEN", SECRET_TOKEN),
    ]
}

#[test]
fn run_without_its_environment_exits_with_2_and_names_what_is_missing() {
    for args in [
        &["run"][..],
        &[][..],
        &["--backend", "synthetic", "run"][..],
    ] {
        let output = agent(args, &[]);
        assert_eq!(code(&output), 2, "{args:?}");
        let text = stderr(&output);
        assert!(
            text.contains("FOLO_CAPTURE_LINK") && text.contains("FOLO_CAPTURE_TOKEN"),
            "{text}"
        );
    }
}

#[test]
fn run_names_only_the_variable_that_is_missing_and_treats_an_empty_one_as_missing() {
    let output = agent(&["run"], &[("FOLO_CAPTURE_LINK", "ws://127.0.0.1:1/x")]);
    assert_eq!(code(&output), 2);
    let text = stderr(&output);
    assert!(
        text.contains("FOLO_CAPTURE_TOKEN") && !text.contains("FOLO_CAPTURE_LINK"),
        "{text}"
    );

    let output = agent(
        &["run"],
        &[
            ("FOLO_CAPTURE_LINK", "ws://127.0.0.1:1/x"),
            ("FOLO_CAPTURE_TOKEN", ""),
        ],
    );
    assert_eq!(code(&output), 2);
    let output = agent(
        &["run"],
        &[("FOLO_CAPTURE_LINK", ""), ("FOLO_CAPTURE_TOKEN", "t")],
    );
    assert_eq!(code(&output), 2);
}

#[test]
fn the_environment_is_checked_before_the_backend() {
    let output = agent(&["--backend", "dda", "run"], &[]);
    assert_eq!(code(&output), 2);
}

/// Whether this build of the agent has the Linux portal backend.
const PORTAL_BUILT: bool = cfg!(all(target_os = "linux", feature = "portal"));

/// The backends that can't be opened here: the Windows ones everywhere, and
/// the platform's own where it has no crate yet.
fn unbuilt_backends() -> Vec<&'static str> {
    let mut backends = vec!["dda", "wgc", "gdi"];
    if !PORTAL_BUILT {
        backends.extend(["auto", "portal"]);
    }
    backends
}

#[test]
fn a_backend_that_is_not_built_yet_exits_with_1_and_says_so() {
    for backend in unbuilt_backends() {
        let output = agent(&["--backend", backend, "run"], &linked());
        assert_eq!(code(&output), 1, "{backend}");
        let text = stderr(&output);
        assert!(
            text.contains("built yet") || text.contains("for Linux") || text.contains("built into"),
            "{backend}: {text}"
        );
        assert!(
            !text.contains(SECRET_TOKEN),
            "the token stays out of the output"
        );
    }
}

#[test]
fn run_with_a_working_backend_stops_at_the_missing_link_client_with_1() {
    let output = agent(&["--backend", "synthetic", "run"], &linked());
    assert_eq!(code(&output), 1);
    let text = stderr(&output);
    assert!(text.contains("capture link isn't built yet"), "{text}");
    assert!(!text.contains(SECRET_TOKEN));
}

#[test]
fn no_command_is_run() {
    assert_eq!(code(&agent(&[], &linked())), 1);
}

#[test]
fn a_replay_file_that_cannot_be_used_exits_with_1() {
    let scratch = Scratch::new();
    let missing = format!("replay:{}", scratch.0.join("missing.folr").display());
    assert_eq!(code(&agent(&["--backend", &missing, "run"], &linked())), 1);
    let garbage = scratch.write("garbage.folr", b"not a replay file");
    let garbage = format!("replay:{}", garbage.display());
    let output = agent(&["--backend", &garbage, "screens", "--json"], &[]);
    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("replay file"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_backend_name_that_does_not_exist_is_a_usage_error() {
    for backend in ["nope", "replay:", "REPLAY:x", "synthetic:1"] {
        let output = agent(&["--backend", backend, "screens", "--json"], &[]);
        assert_eq!(code(&output), 2, "{backend}");
        assert!(stderr(&output).contains("--backend"), "{}", stderr(&output));
    }
    assert_eq!(code(&agent(&["screens", "--bogus"], &[])), 2);
    assert_eq!(code(&agent(&["capture"], &[])), 2);
}

#[test]
fn help_and_version_exit_with_0() {
    assert_eq!(code(&agent(&["--help"], &[])), 0);
    assert_eq!(code(&agent(&["screens", "--help"], &[])), 0);
    let output = agent(&["--version"], &[]);
    assert_eq!(code(&output), 0);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("folo-capture "));
}

#[test]
fn screens_json_on_the_synthetic_backend_lists_its_one_screen() {
    let output = agent(&["screens", "--json", "--backend", "synthetic"], &[]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        listed,
        json!({"screens": [{"id": "synthetic-0", "name": "Synthetic screen", "width": 1920, "height": 1080, "primary": true}]})
    );
    assert!(
        output.stdout.ends_with(b"\n")
            && !output.stdout[..output.stdout.len() - 1].contains(&b'\n'),
        "one line"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn screens_json_on_a_replay_file_lists_the_size_of_its_first_frame() {
    let scratch = Scratch::new();
    let backend = format!("replay:{}", replay_file(&scratch).display());
    let output = agent(&["--backend", &backend, "screens", "--json"], &[]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        listed,
        json!({"screens": [{"id": "replay", "name": "Replay", "width": 16, "height": 9, "primary": true}]})
    );
}

#[test]
fn screens_needs_no_environment_and_captures_nothing() {
    let output = agent(&["screens", "--json", "--backend", "synthetic"], &[]);
    assert_eq!(code(&output), 0);
}

#[test]
fn screens_without_json_prints_a_line_per_screen() {
    let output = agent(&["screens", "--backend", "synthetic"], &[]);
    assert_eq!(code(&output), 0);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(text, "synthetic-0\tSynthetic screen\t1920x1080 (primary)\n");
}

#[test]
fn screens_on_a_backend_that_is_not_built_exits_with_1() {
    for backend in unbuilt_backends() {
        let output = agent(&["screens", "--json", "--backend", backend], &[]);
        assert_eq!(code(&output), 1, "{backend}");
        assert_eq!(output.stdout, b"", "{backend}");
        let text = stderr(&output);
        assert!(
            text.contains("built yet") || text.contains("for Linux") || text.contains("built into"),
            "{backend}: {text}"
        );
    }
}

#[cfg(all(target_os = "linux", feature = "portal"))]
#[test]
fn screens_json_on_linux_lists_the_portal_without_asking_it_anything() {
    // No session bus is reachable in this environment: listing must not need one.
    for backend in ["portal", "auto"] {
        let output = agent(
            &["screens", "--json", "--backend", backend],
            &[("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")],
        );
        assert_eq!(code(&output), 0, "{backend}: {}", stderr(&output));
        let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            listed["screens"],
            json!([{"id": "portal", "name": "Screen chosen in the desktop's dialog", "width": 0, "height": 0, "primary": true}]),
            "{backend}"
        );
        assert_eq!(stderr(&output), "");
    }
}

#[cfg(all(target_os = "linux", feature = "portal"))]
#[test]
fn run_on_the_portal_backend_stops_at_the_missing_link_client_like_the_others() {
    let output = agent(&["--backend", "portal", "run"], &linked());
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("capture link isn't built yet"));
}

#[test]
fn the_default_backend_is_auto() {
    let output = agent(&["screens", "--json"], &[]);
    assert_eq!(code(&output), i32::from(!PORTAL_BUILT));
}
