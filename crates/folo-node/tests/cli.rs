//! The `folo` binary's command line (docs/architecture/control-plane.md#command-line):
//! exit statuses, and `folo status` from the home's files when no node runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use folo_protocol::generated::NodeStatus;
use serde_json::Value;

const FOLO: &str = env!("CARGO_BIN_EXE_folo");

/// A fresh home, removed afterwards.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-cli-test-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `folo --home <home> <args>`, run in the home without the caller's
/// `FOLO_*` variables, so no `studio.json` or override comes from outside.
fn folo(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(FOLO);
    command.arg("--home").arg(home).args(args).current_dir(home);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("FOLO_") {
            command.env_remove(name);
        }
    }
    command.output().unwrap()
}

fn status_code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn entries(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn status_without_a_node_reads_the_files_and_changes_nothing() {
    let home = Home::new();
    let output = folo(&home.0, &["status", "--json"]);
    assert_eq!(status_code(&output), 3, "{output:?}");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    serde_json::from_value::<NodeStatus>(status.clone()).expect("a NodeStatus");
    assert_eq!(status["running"], false);
    assert_eq!(status["id"], "");
    assert_eq!(status["mode"], "custom");
    assert_eq!(status["launch"], "foreground");
    assert_eq!(status["listen"]["address"], "127.0.0.1");
    assert_eq!(status["listen"]["port"], 8777);
    assert_eq!(status["workers"], serde_json::json!({}));
    assert_eq!(entries(&home.0), Vec::<String>::new());

    let output = folo(&home.0, &["status", "--port", "9000"]);
    assert_eq!(status_code(&output), 3);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("not running"), "{text}");
    assert!(text.contains("listen    127.0.0.1:9000"), "{text}");
}

#[test]
fn reload_without_a_node_exits_with_3() {
    let home = Home::new();
    assert_eq!(status_code(&folo(&home.0, &["reload"])), 3);
}

#[test]
fn import_without_a_studio_json_fails() {
    let home = Home::new();
    let output = folo(&home.0, &["config", "import"]);
    assert_eq!(status_code(&output), 1, "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("studio.json"));
}

#[test]
fn later_commands_say_so_and_usage_errors_exit_with_2() {
    let home = Home::new();
    for args in [
        &["token", "show"][..],
        &["pair", "reset"],
        &["join", "code"],
    ] {
        let output = folo(&home.0, args);
        assert_eq!(status_code(&output), 1, "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("arrives with RB-1"),
            "{args:?}"
        );
    }
    for args in [&["--bogus"][..], &["--port", "9000", "status"]] {
        assert_eq!(status_code(&folo(&home.0, args)), 2, "{args:?}");
    }
}
