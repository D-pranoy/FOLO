//! The built `folo-capture` against a fake node (ADR-0037 D2, D22): it dials
//! with the launch token from its environment, captures from the synthetic
//! backend, and ends with the exit statuses of the link. No real screen.

mod common;

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{FakeNode, TOKEN};
use serde_json::json;

const FOLO_CAPTURE: &str = env!("CARGO_BIN_EXE_folo-capture");

fn start(node: &FakeNode, backend: &str) -> Child {
    let mut command = Command::new(FOLO_CAPTURE);
    command.args(["--backend", backend, "run"]);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("FOLO_") {
            command.env_remove(name);
        }
    }
    command
        .env("FOLO_CAPTURE_LINK", &node.url)
        .env("FOLO_CAPTURE_TOKEN", TOKEN)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Waits for the agent to exit; its status and what it printed.
async fn finished(child: Child) -> (i32, String, String) {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || child.wait_with_output().unwrap()),
    )
    .await
    .expect("the agent exits")
    .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn start_params() -> serde_json::Value {
    json!({
        "screen": "synthetic-0", "width": 64, "height": 36, "rate_hz": 30,
        "black_bars": {"preset": "auto", "mode": "default", "threshold": 0.05},
        "profile": "video",
    })
}

#[tokio::test]
async fn the_agent_captures_the_synthetic_screen_for_its_node_and_ends_on_restart() {
    let node = FakeNode::bind().await;
    let child = start(&node, "synthetic");
    let mut link = node.accept().await;
    link.welcomed().await;
    assert_eq!(
        link.call("capture.start", start_params()).await["result"],
        json!({})
    );
    let frame = link.frame().await;
    assert_eq!((frame.picture.width, frame.picture.height), (64, 36));
    assert_eq!(
        link.call("capture.stop", json!({})).await["result"],
        json!({})
    );
    link.call("restart", json!({"component": "capture"})).await;
    let (status, stdout, stderr) = finished(child).await;
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "");
    assert!(
        !stderr.contains(TOKEN),
        "the launch token is in the log: {stderr}"
    );
    assert!(
        stderr
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()),
        "every log line is a JSON record: {stderr}"
    );
}

#[tokio::test]
async fn a_rejected_agent_ends_with_status_one() {
    let node = FakeNode::bind().await;
    let child = start(&node, "synthetic");
    let mut link = node.accept().await;
    link.hello().await;
    link.welcome("rejected").await;
    let (status, _, stderr) = finished(child).await;
    assert_eq!(status, 1, "{stderr}");
    assert!(!stderr.contains(TOKEN));
}
