//! The node end to end, in this process, with a fake effect engine (RB-06):
//! it starts, takes its home, makes its identity, serves its loopback
//! routes and the local link, supervises the engine, reloads and stops.
//!
//! The fake engine is this test binary run again by the node: the tests
//! named `fake_engine_*` act as an engine when the node's link variables
//! are set, and do nothing otherwise.

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folo_node::cli::{LocationArgs, RunArgs};
use folo_node::client;
use folo_protocol::generated::{NodeStatus, ReloadResult};
use folo_supervisor::EngineCommand;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest as _};

// ------------------------------------------------------------------ the fake engine

/// How a fake engine behaves once linked.
#[derive(Clone, Copy)]
enum Fake {
    /// Sends `status` twice a second and answers requests.
    Normal,
    /// Falls silent.
    Hang,
}

fn fake_engine(mode: Fake) {
    let (Ok(url), Ok(token)) = (
        std::env::var("FOLO_SOURCE_LINK"),
        std::env::var("FOLO_SOURCE_TOKEN"),
    ) else {
        return; // an ordinary test run
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let status = runtime.block_on(engine(&url, &token, mode));
    std::process::exit(status);
}

fn rpc(value: &Value) -> Message {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(value.to_string().as_bytes());
    Message::Binary(bytes.into())
}

async fn engine(url: &str, token: &str, mode: Fake) -> i32 {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "folo.source-link.v1".parse().unwrap(),
    );
    let Ok((mut socket, _)) = tokio_tungstenite::connect_async(request).await else {
        return 1;
    };
    let hello = json!({
        "version": ["1.2"],
        "node": {"id": "folo-source", "name": "fake", "version": "0", "platform": "test"},
        "purpose": {"kind": "local", "token": token},
    });
    socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .unwrap();
    let Some(Ok(Message::Text(welcome))) = socket.next().await else {
        return 1;
    };
    let welcome: Value = serde_json::from_str(&welcome).unwrap();
    if welcome["result"] != "ok" {
        return 1;
    }
    if let Fake::Hang = mode {
        tokio::time::sleep(Duration::from_secs(600)).await;
        return 0;
    }
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let status = json!({"jsonrpc": "2.0", "method": "status", "params": {"state": "running", "workers": {}, "inputs": {}, "errors": []}});
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if socket.send(rpc(&status)).await.is_err() {
                    return 1;
                }
            }
            message = socket.next() => {
                let Some(Ok(Message::Binary(bytes))) = message else {
                    return 1;
                };
                let request: Value = serde_json::from_slice(&bytes[1..]).unwrap();
                let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": {}});
                socket.send(rpc(&reply)).await.unwrap();
                if request["method"] == "restart" {
                    let _ = socket.close(None).await;
                    return 0;
                }
            }
        }
    }
}

#[test]
fn fake_engine_normal() {
    fake_engine(Fake::Normal);
}

#[test]
fn fake_engine_hang() {
    fake_engine(Fake::Hang);
}

fn fake(name: &str) -> EngineCommand {
    EngineCommand {
        program: std::env::current_exe().unwrap(),
        args: [name, "--exact", "--nocapture", "--test-threads=1"]
            .map(Into::into)
            .to_vec(),
    }
}

// ------------------------------------------------------------------ helpers

/// The node's logger is global to the process, so nodes in this process
/// run one at a time.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fresh home, removed afterwards.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-node-test-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn location(&self) -> LocationArgs {
        LocationArgs {
            home: Some(self.0.clone()),
            service: false,
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn run_args(port: u16) -> RunArgs {
    RunArgs {
        port: Some(port.to_string()),
        ..RunArgs::default()
    }
}

/// A node running on its own task; `stop` ends it, and `done` gives its
/// exit status.
struct Running {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<u8>,
}

impl Running {
    fn start(home: &Home, args: RunArgs, engine: Option<EngineCommand>) -> Self {
        let port = args.port.as_deref().unwrap().parse().unwrap();
        let location = home.location();
        let (stop, stopped) = oneshot::channel::<()>();
        let done = tokio::spawn(async move {
            folo_node::node::run_with(&location, &args, engine, async {
                let _ = stopped.await;
            })
            .await
        });
        Self {
            port,
            stop: Some(stop),
            done,
        }
    }

    async fn status(&self) -> Option<Value> {
        let port = self.port;
        let answer = tokio::task::spawn_blocking(move || {
            client::request(port, "GET", "/api/v1/system/status", None)
        })
        .await
        .unwrap();
        match answer {
            Ok((200, body)) => serde_json::from_str(&body).ok(),
            _ => None,
        }
    }

    /// Waits until the status satisfies `wanted`.
    async fn wait_for(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = self.status().await
                && wanted(&status)
            {
                return status;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn stop(mut self) -> u8 {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(30), self.done)
            .await
            .expect("the node stops")
            .unwrap()
    }
}

fn engine_state(status: &Value) -> &str {
    status["workers"]["folo-source"]["state"]
        .as_str()
        .unwrap_or_default()
}

fn request(port: u16, raw: &str) -> String {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}

fn exists(path: &Path) -> bool {
    path.try_exists().unwrap_or(false)
}

// ------------------------------------------------------------------ the tests

#[tokio::test(flavor = "multi_thread")]
async fn the_node_runs_supervises_its_engine_and_stops() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    let status = node
        .wait_for("the engine to link", |s| engine_state(s) == "running")
        .await;
    serde_json::from_value::<NodeStatus>(status.clone()).expect("a NodeStatus");
    assert_eq!(status["running"], true);
    assert_eq!(status["role"], "controller");
    assert_eq!(status["launch"], "foreground");
    assert_eq!(status["mode"], "custom");
    assert_eq!(status["id"].as_str().unwrap().len(), 16);
    assert_eq!(status["listen"]["address"], "127.0.0.1");
    assert_eq!(status["overrides"][0]["name"], "--port");
    assert!(status["workers"]["folo-source"]["pid"].as_u64().unwrap() > 0);
    assert!(exists(&home.0.join("folo.lock")));
    assert!(exists(&home.0.join("secrets.json")));
    let log = std::fs::read_to_string(home.0.join("logs").join("folo.log")).unwrap();
    assert!(
        log.lines()
            .all(|line| serde_json::from_str::<Value>(line).is_ok()),
        "{log}"
    );
    assert!(log.contains("made the node's identity"), "{log}");

    let port = node.port;
    let reloaded = tokio::task::spawn_blocking(move || {
        client::request(port, "POST", "/api/v1/system/reload", Some("{}"))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reloaded.0, 200);
    serde_json::from_str::<ReloadResult>(&reloaded.1).expect("a ReloadResult");
    assert_eq!(
        serde_json::from_str::<Value>(&reloaded.1).unwrap(),
        json!({"applied": [], "restart_required": []})
    );

    let id = status["id"].clone();
    assert_eq!(node.stop().await, 0);
    assert_eq!(folo_node::lock::holder(&home.0.join("folo.lock")), None);

    // The identity survives a restart.
    let node = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    let status = node.wait_for("the node", |_| true).await;
    assert_eq!(status["id"], id);
    assert_eq!(node.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_node_per_home() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let first = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    first.wait_for("the first node", |_| true).await;
    let second = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), second.done)
            .await
            .unwrap()
            .unwrap(),
        3
    );
    assert_eq!(first.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hung_engine_is_killed_and_started_again() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, run_args(free_port()), Some(fake("fake_engine_hang")));
    node.wait_for("the engine to link", |s| engine_state(s) == "running")
        .await;
    let status = node
        .wait_for("a restart", |s| {
            s["workers"]["folo-source"]["restarts"].as_u64() >= Some(1)
        })
        .await;
    assert_ne!(engine_state(&status), "stopped");
    assert_eq!(node.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_the_local_source_off_stops_the_engine() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    node.wait_for("the engine to link", |s| engine_state(s) == "running")
        .await;
    std::fs::write(
        home.0.join("node.toml"),
        "[local_source]\nenabled = false\n",
    )
    .unwrap();
    let port = node.port;
    let reloaded = tokio::task::spawn_blocking(move || {
        client::request(port, "POST", "/api/v1/system/reload", Some("{}"))
    })
    .await
    .unwrap()
    .unwrap();
    let result: Value = serde_json::from_str(&reloaded.1).unwrap();
    assert_eq!(result["applied"], json!(["local_source.enabled"]));
    let status = node
        .wait_for("the engine to stop", |s| engine_state(s) == "stopped")
        .await;
    assert_eq!(status["workers"]["folo-source"]["pid"], 0);
    assert_eq!(status["workers"]["folo-source"]["restarts"], 0);
    assert_eq!(node.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_routes_check_the_host_and_json() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let args = RunArgs {
        local_source: Some("off".into()),
        ..run_args(free_port())
    };
    let node = Running::start(&home, args, None);
    node.wait_for("the node", |_| true).await;
    let port = node.port;
    let answers = tokio::task::spawn_blocking(move || {
        [
            request(port, "GET /api/v1/system/status HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n"),
            request(port, "POST /api/v1/system/reload HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"),
            request(port, "GET /api/v1/nothing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
            request(port, "DELETE /api/v1/system/status HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
            request(port, "POST /api/v1/system/reload HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 7\r\nConnection: close\r\n\r\n{\"a\":1}"),
            request(port, "TRACE /api/v1/system/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
            request(port, "PUT /api/v1/system/status HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        ]
    })
    .await
    .unwrap();
    assert!(answers[0].starts_with("HTTP/1.1 403"), "{}", answers[0]);
    assert!(answers[0].contains("host_not_allowed"), "{}", answers[0]);
    assert!(answers[1].starts_with("HTTP/1.1 415"), "{}", answers[1]);
    assert!(answers[2].starts_with("HTTP/1.1 404"), "{}", answers[2]);
    assert!(
        answers[2].contains("\"code\":\"not_found\""),
        "{}",
        answers[2]
    );
    assert!(answers[3].starts_with("HTTP/1.1 405"), "{}", answers[3]);
    assert!(
        answers[3].to_ascii_lowercase().contains("allow: get"),
        "{}",
        answers[3]
    );
    assert!(answers[4].starts_with("HTTP/1.1 400"), "{}", answers[4]);
    assert!(
        answers[4].contains("\"code\":\"bad_body\""),
        "{}",
        answers[4]
    );
    // Methods a route doesn't take get 405 before the JSON rule
    for answer in &answers[5..] {
        assert!(answer.starts_with("HTTP/1.1 405"), "{answer}");
    }
    assert_eq!(node.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_local_link_refuses_other_tokens() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    node.wait_for("the engine to link", |s| engine_state(s) == "running")
        .await;
    let url = format!("ws://127.0.0.1:{}/api/v1/source-link", node.port);
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "folo.source-link.v1".parse().unwrap(),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(
        response.headers()["sec-websocket-protocol"],
        "folo.source-link.v1"
    );
    let hello =
        json!({"version": ["1.2"], "node": {}, "purpose": {"kind": "local", "token": "guess"}});
    socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .unwrap();
    let Some(Ok(Message::Text(welcome))) = socket.next().await else {
        panic!("no welcome");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&welcome).unwrap()["result"],
        "rejected"
    );
    let status = node.status().await.unwrap();
    assert_eq!(
        engine_state(&status),
        "running",
        "the real engine stays linked"
    );
    assert_eq!(node.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_document_the_node_cant_read_stops_it() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    std::fs::write(home.0.join("zones.json"), b"[]").unwrap();
    let node = Running::start(
        &home,
        run_args(free_port()),
        Some(fake("fake_engine_normal")),
    );
    let status = tokio::time::timeout(Duration::from_secs(30), node.done)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, 1);
    assert_eq!(std::fs::read(home.0.join("zones.json")).unwrap(), b"[]");
}

#[tokio::test(flavor = "multi_thread")]
async fn managed_launches_take_no_other_flags() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let args = RunArgs {
        managed: true,
        ..run_args(free_port())
    };
    let status = folo_node::node::run_with(&home.location(), &args, None, async {}).await;
    assert_eq!(status, 2);
}
