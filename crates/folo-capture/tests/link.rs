//! The agent's link client against a fake node (ADR-0037 D22): the
//! handshake, requests and their error codes, status, frames, reconnecting,
//! giving up, and the exit statuses of D2. The agent captures from the
//! synthetic backend, so no real screen is involved.

use std::time::{Duration, Instant};

mod common;

use folo_capture::cli::{EXIT_FAILED, EXIT_OK};
use folo_capture::environment::Environment;
use folo_capture::link::{self, Timing};
use folo_capture::worker::{self, Agent};
use folo_capture_core::clock::ProcessClock;
use folo_capture_core::synthetic::{Pattern, SyntheticBackend};
use futures_util::SinkExt as _;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use common::{FakeNode, TOKEN};

fn fast() -> Timing {
    Timing {
        backoff_first: Duration::from_millis(30),
        backoff_max: Duration::from_millis(120),
        jitter: 0.25,
        give_up: Duration::from_millis(800),
        handshake: Duration::from_secs(2),
        status_every: Duration::from_millis(100),
        send: Duration::from_secs(2),
    }
}

fn start_params(rate: u32) -> Value {
    json!({
        "screen": "synthetic-0", "width": 64, "height": 36, "rate_hz": rate,
        "black_bars": {"preset": "auto", "mode": "default", "threshold": 0.05},
        "profile": "video",
    })
}

/// Runs the agent (synthetic backend) against `environment`; its exit status
/// is the task's result.
fn run_agent(environment: Environment, timing: Timing) -> tokio::task::JoinHandle<u8> {
    tokio::spawn(async move {
        let backend = SyntheticBackend::new(Pattern::Letterbox);
        let (handle, thread) =
            worker::spawn(Agent::new(Box::new(backend), Box::new(ProcessClock::new())));
        let status = link::run(&environment, &handle, timing).await;
        drop(handle);
        let _ = thread.join();
        status
    })
}

async fn exited(agent: tokio::task::JoinHandle<u8>) -> u8 {
    tokio::time::timeout(Duration::from_secs(10), agent)
        .await
        .expect("the agent exits")
        .unwrap()
}

#[tokio::test]
async fn the_agent_dials_with_the_subprotocol_and_its_hello() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    assert_eq!(link.protocol.as_deref(), Some("folo.capture.v1"));
    let hello = link.hello().await;
    assert_eq!(hello["version"], json!(["1"]));
    assert_eq!(hello["token"], TOKEN);
    assert!(hello["agent"]["version"].is_string());
    assert!(
        ["windows", "linux"].contains(&hello["agent"]["platform"].as_str().unwrap())
            || cfg!(not(any(windows, target_os = "linux")))
    );
    link.welcome("ok").await;
    // It says what it is at once: off
    let status = link.status().await;
    assert_eq!(status["state"], "off");
    // Ending the agent: a restart
    assert_eq!(
        link.call("restart", json!({"component": "capture"})).await["result"],
        json!({})
    );
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn a_rejection_ends_the_agent_with_status_one() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.hello().await;
    link.welcome("rejected").await;
    assert_eq!(exited(agent).await, EXIT_FAILED);
}

#[tokio::test]
async fn it_captures_on_request_and_is_off_before_and_after() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.welcomed().await;
    assert_eq!(link.status().await["state"], "off");

    assert_eq!(
        link.call("capture.start", start_params(30)).await["result"],
        json!({})
    );
    let capturing = link.status_where(|s| s["state"] == "capturing").await;
    assert_eq!(capturing["backend"], "synthetic");
    assert_eq!(capturing["screen"], "synthetic-0");
    assert!(
        capturing["bars"]["top"].as_f64().unwrap() > 0.0,
        "the letterbox's bars"
    );
    let first = link.frame().await;
    assert_eq!((first.picture.width, first.picture.height), (64, 36));
    let second = link.frame().await;
    assert!(second.sequence > first.sequence);

    // A change applies in place: no restart of the session
    assert_eq!(
        link.call("capture.start", start_params(10)).await["result"],
        json!({})
    );
    link.frame().await;

    assert_eq!(
        link.call("capture.stop", json!({})).await["result"],
        json!({})
    );
    link.status_where(|s| s["state"] == "off").await;
    // Nothing after it: frames in flight before the stop were read above
    link.frames.clear();
    let silence = tokio::time::timeout(Duration::from_millis(400), async {
        loop {
            if link.read().await?.is_err() {
                return Some(());
            }
        }
    })
    .await;
    assert!(silence.is_err(), "a frame came after capture.stop");

    link.call("restart", json!({"component": "capture"})).await;
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn it_sends_its_status_at_least_every_interval_when_idle() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.welcomed().await;
    link.status().await; // the one at once
    let start = Instant::now();
    for _ in 0..4 {
        let status = tokio::time::timeout(Duration::from_millis(400), link.status())
            .await
            .expect("a status within the interval and some slack");
        assert_eq!(status["state"], "off");
    }
    assert!(
        start.elapsed() >= Duration::from_millis(250),
        "not faster than asked"
    );
    link.call("restart", json!({"component": "capture"})).await;
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn requests_are_answered_with_the_codes_of_the_link() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.welcomed().await;

    let listed = link.call("screens.list", json!({})).await;
    assert_eq!(listed["result"]["screens"][0]["id"], "synthetic-0");
    assert_eq!(
        link.call("no.such.method", json!({})).await["error"]["code"],
        -32601
    );
    let unknown = link
        .call("capture.start", json!({"screen": "nowhere", "width": 64, "height": 36, "rate_hz": 30, "black_bars": {"preset": "auto", "mode": "default", "threshold": 0.05}, "profile": "video"}))
        .await;
    assert_eq!(unknown["error"]["code"], -32000);
    assert_eq!(
        link.call("capture.start", json!({"screen": "synthetic-0"}))
            .await["error"]["code"],
        -32602
    );
    // Another component: -32602, and the agent stays
    assert_eq!(
        link.call("restart", json!({"component": "engine"})).await["error"]["code"],
        -32602
    );
    assert_eq!(
        link.call("restart", json!({})).await["error"]["code"],
        -32602
    );
    assert_eq!(
        link.call("screens.list", json!({})).await["result"]["screens"][0]["id"],
        "synthetic-0"
    );

    assert_eq!(
        link.call("restart", json!({"component": "capture"})).await["result"],
        json!({})
    );
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn losing_the_link_stops_the_capture_and_it_comes_back_off() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.welcomed().await;
    link.call("capture.start", start_params(30)).await;
    link.status_where(|s| s["state"] == "capturing").await;
    drop(link);

    let mut again = node.accept().await;
    again.welcomed().await;
    // Off at once, and no frames until it is told to capture again
    assert_eq!(again.status().await["state"], "off");
    let frames = tokio::time::timeout(Duration::from_millis(400), async {
        loop {
            if again.read().await?.is_err() {
                return Some(());
            }
        }
    })
    .await;
    assert!(frames.is_err(), "frames after a reconnect, before a start");
    again.call("restart", json!({"component": "capture"})).await;
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn without_a_node_it_exits_with_status_one_after_the_give_up_time() {
    let node = FakeNode::bind().await;
    let environment = node.environment();
    drop(node); // nothing listens any more
    let timing = fast();
    let started = Instant::now();
    let agent = run_agent(environment, timing);
    assert_eq!(exited(agent).await, EXIT_FAILED);
    let took = started.elapsed();
    assert!(took >= timing.give_up, "{took:?}");
    assert!(took < timing.give_up + Duration::from_secs(2), "{took:?}");
}

#[tokio::test]
async fn the_give_up_time_counts_from_the_loss_and_a_link_resets_it() {
    let node = FakeNode::bind().await;
    let timing = fast();
    let agent = run_agent(node.environment(), timing);
    // Link, lose it, link again after most of the give-up time, lose it again
    let mut link = node.accept().await;
    link.welcomed().await;
    drop(link);
    tokio::time::sleep(timing.give_up / 2).await;
    let mut link = node.accept().await;
    link.welcomed().await;
    drop(link);
    let lost_at = Instant::now();
    // The listener stays but nothing accepts: connections hang in the backlog
    // until the handshake times out; the agent gives up give_up after the loss
    let status = exited(agent).await;
    assert_eq!(status, EXIT_FAILED);
    assert!(lost_at.elapsed() + Duration::from_millis(100) >= timing.give_up);
}

#[tokio::test]
async fn a_message_over_the_limit_drops_the_link_and_the_agent_comes_back() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.welcomed().await;
    let mut big = vec![0x01];
    big.resize(400_001, b' ');
    let _ = link.socket.send(Message::binary(big)).await;
    let mut again = node.accept().await;
    again.welcomed().await;
    again.call("restart", json!({"component": "capture"})).await;
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn a_node_that_stops_reading_gets_the_newest_frame_not_all_of_them() {
    let node = FakeNode::bind().await;
    let agent = run_agent(node.environment(), fast());
    let mut link = node.accept().await;
    link.welcomed().await;
    // The largest frames at the highest rate: more than the sockets hold
    let mut large = start_params(60);
    large["width"] = json!(480);
    large["height"] = json!(270);
    link.call("capture.start", large).await;
    let first = link.frame().await;
    // The node reads nothing for a while
    tokio::time::sleep(Duration::from_millis(900)).await;
    let mut sequences = vec![first.sequence];
    while sequences.len() < 25 {
        sequences.push(link.frame().await.sequence);
    }
    let span = sequences.last().unwrap() - sequences[0] + 1;
    let received = u32::try_from(sequences.len()).unwrap();
    assert!(
        span > received + 10,
        "frames were queued instead of replaced: {received} frames over {span} numbers"
    );
    link.call("restart", json!({"component": "capture"})).await;
    assert_eq!(exited(agent).await, EXIT_OK);
}

#[tokio::test]
async fn a_link_that_is_not_on_this_machine_is_refused_before_the_token_is_sent() {
    let environment = Environment {
        link: "ws://192.0.2.1:9/api/v1/capture-link".into(),
        token: TOKEN.into(),
    };
    let agent = run_agent(environment, fast());
    assert_eq!(exited(agent).await, EXIT_FAILED);
}
