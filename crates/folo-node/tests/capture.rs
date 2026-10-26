//! The node's side of the capture link end to end (RB-14, ADR-0037): the
//! node starts a fake agent while capture is wanted and stops it when it
//! isn't, answers its handshake, forwards its screen frames unchanged to a
//! fake engine, and keeps raw frames out of everything else.
//!
//! The fake engine and the fake agent are this test binary run again by the
//! node: the tests named `fake_*` act as the engine or the agent when the
//! node's link variables are set, and do nothing otherwise. They talk to the
//! test through files in a folder of the temporary folder named after the
//! node's port (the node removes `FOLO_*` variables from their environment).
//! That folder is outside the node's home, so it may hold raw frames: it
//! stands in for the engine's memory.

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folo_capture_core::codec;
use folo_capture_core::types::{Bars, FrameFlags, Picture, ScreenFrame};
use folo_node::cli::{LocationArgs, RunArgs};
use folo_node::client;
use folo_supervisor::EngineCommand;
use folo_supervisor::capture::CaptureCommand;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

// ------------------------------------------------------------------ frames

/// The pixel every frame is filled with, apart from the first.
const MARKER: [u8; 3] = [0xA5, 0x5A, 0xC3];
const WIDTH: u16 = 32;
const HEIGHT: u16 = 18;

/// A run of marker pixels no other part of the node may ever hold.
fn marker_run() -> Vec<u8> {
    MARKER.repeat(40)
}

/// The frame message (type byte `0x04`) with this sequence number: marker
/// pixels everywhere, protected and held flags on some, bars at the top and bottom.
fn frame_message(sequence: u32) -> Vec<u8> {
    let mut pixels = MARKER.repeat(usize::from(WIDTH) * usize::from(HEIGHT));
    pixels[..3].copy_from_slice(&sequence.to_le_bytes()[..3]);
    let flags = FrameFlags {
        protected: sequence % 5 == 1,
        hdr: false,
        held: sequence % 5 == 2,
    };
    let frame = ScreenFrame {
        sequence,
        timestamp_us: u64::from(sequence) * 1000,
        picture: Picture {
            screen_index: 0,
            width: WIDTH,
            height: HEIGHT,
            flags,
            bars: Bars {
                top: 2,
                bottom: 2,
                left: 0,
                right: 0,
            },
            pixels,
        },
    };
    let mut message = vec![codec::MESSAGE_TYPE];
    message.extend(codec::encode(&frame).unwrap());
    message
}

// ------------------------------------------------------------------ the fakes

fn record_dir(port: u16) -> PathBuf {
    std::env::temp_dir().join(format!("folo-capture-record-{port}"))
}

/// The port in a link URL (`ws://127.0.0.1:<port>/api/v1/...`).
fn port_of(url: &str) -> u16 {
    url.split("127.0.0.1:")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|port| port.parse().ok())
        .expect("a loopback link URL")
}

fn rpc(value: &Value) -> Message {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(value.to_string().as_bytes());
    Message::Binary(bytes.into())
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) {
    let temporary = dir.join(format!("{name}.tmp"));
    std::fs::write(&temporary, bytes).unwrap();
    std::fs::rename(&temporary, dir.join(name)).unwrap();
}

fn append_line(dir: &Path, name: &str, line: &str) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

/// The engine: links, sends its heartbeat, answers requests, and records
/// every screen frame message it is sent, as it came.
async fn engine(url: &str, token: &str) -> i32 {
    let dir = record_dir(port_of(url));
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "folo.source-link.v1".parse().unwrap(),
    );
    let Ok((mut socket, _)) = tokio_tungstenite::connect_async(request).await else {
        return 1;
    };
    let hello = json!({
        "version": ["1.3"],
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
    if serde_json::from_str::<Value>(&welcome).unwrap()["result"] != "ok" {
        return 1;
    }
    let status = json!({"jsonrpc": "2.0", "method": "status", "params": {"state": "running", "workers": {}, "inputs": {}, "errors": []}});
    let mut tick = tokio::time::interval(Duration::from_millis(400));
    let mut frames = 0u64;
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
                if bytes[0] == codec::MESSAGE_TYPE {
                    frames += 1;
                    write_file(&dir, "engine-latest.bin", &bytes);
                    write_file(&dir, "engine-frames", frames.to_string().as_bytes());
                    continue;
                }
                let request: Value = serde_json::from_slice(&bytes[1..]).unwrap();
                let result = match request["method"].as_str() {
                    Some("inputs.list") => json!({
                        "default_output": "", "outputs": [], "inputs": [], "apps": [],
                        "screens": [], "media": true,
                    }),
                    _ => json!({}),
                };
                let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
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
fn fake_engine_recording() {
    let (Ok(url), Ok(token)) = (
        std::env::var("FOLO_SOURCE_LINK"),
        std::env::var("FOLO_SOURCE_TOKEN"),
    ) else {
        return; // an ordinary test run
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    std::process::exit(runtime.block_on(engine(&url, &token)));
}

/// How a fake agent behaves once linked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fake {
    /// Captures on request, ends on `restart`.
    Normal,
    /// Like Normal, and reports a new restore token once it captures.
    Token,
    /// Links, then says nothing.
    Hang,
    /// Like Normal, but ignores `restart`.
    Stubborn,
    /// Sends a message over the link's limit.
    Oversize,
}

async fn agent(url: &str, token: &str, mode: Fake) -> i32 {
    let dir = record_dir(port_of(url));
    write_file(&dir, "agent-token", token.as_bytes());
    write_file(&dir, "agent-pid", std::process::id().to_string().as_bytes());
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "folo.capture.v1".parse().unwrap());
    let Ok((mut socket, _)) = tokio_tungstenite::connect_async(request).await else {
        return 1;
    };
    let hello =
        json!({"version": ["1"], "agent": {"version": "0", "platform": "test"}, "token": token});
    socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .unwrap();
    let Some(Ok(Message::Text(welcome))) = socket.next().await else {
        return 1;
    };
    if serde_json::from_str::<Value>(&welcome).unwrap()["result"] != "ok" {
        return 1;
    }
    match mode {
        Fake::Hang => {
            tokio::time::sleep(Duration::from_secs(600)).await;
            return 0;
        }
        Fake::Oversize => {
            let mut big = vec![0x01];
            big.resize(400_001, b' ');
            let _ = socket.send(Message::Binary(big.into())).await;
            // The node ends the link: the next read says so
            while let Some(Ok(_)) = socket.next().await {}
            write_file(&dir, "oversize-closed", b"1");
            tokio::time::sleep(Duration::from_secs(600)).await;
            return 0;
        }
        _ => {}
    }
    capture_loop(&mut socket, &dir, mode).await
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn capture_loop(socket: &mut Socket, dir: &Path, mode: Fake) -> i32 {
    let (mut state, mut screen) = ("off", String::new());
    let mut sequence = 0u32;
    let mut new_token = false;
    let mut status_tick = tokio::time::interval(Duration::from_millis(400));
    let mut frame_tick = tokio::time::interval(Duration::from_millis(33));
    loop {
        tokio::select! {
            _ = status_tick.tick() => {
                let mut params = json!({
                    "state": state, "backend": "fake", "screen": screen, "fps": 30,
                    "hdr": false, "protected": false,
                    "bars": {"top": 0.1, "bottom": 0.1, "left": 0, "right": 0},
                });
                if new_token && state == "capturing" {
                    params["restore_token"] = json!("fresh-token");
                    new_token = false;
                }
                let status = json!({"jsonrpc": "2.0", "method": "capture.status", "params": params});
                if socket.send(rpc(&status)).await.is_err() {
                    return 1;
                }
            }
            _ = frame_tick.tick(), if state == "capturing" => {
                sequence += 1;
                if socket.send(Message::Binary(frame_message(sequence).into())).await.is_err() {
                    return 1;
                }
            }
            message = socket.next() => {
                let Some(Ok(Message::Binary(bytes))) = message else {
                    return 1;
                };
                let request: Value = serde_json::from_slice(&bytes[1..]).unwrap();
                let params = &request["params"];
                let mut exit = None;
                let answer = match request["method"].as_str().unwrap_or_default() {
                    "screens.list" => Ok(json!({"screens": [
                        {"id": "fake-0", "name": "Fake", "width": 1920, "height": 1080, "primary": true},
                    ]})),
                    "capture.start" => {
                        append_line(dir, "agent-starts.jsonl", &params.to_string());
                        if params["screen"] == "fake-0" {
                            (state, screen) = ("capturing", "fake-0".to_owned());
                            new_token = mode == Fake::Token;
                            Ok(json!({}))
                        } else {
                            Err((-32000, "no such screen"))
                        }
                    }
                    "capture.stop" => {
                        append_line(dir, "agent-events", "stop");
                        (state, screen) = ("off", String::new());
                        Ok(json!({}))
                    }
                    "restart" if params["component"] == "capture" => {
                        append_line(dir, "agent-events", "restart");
                        if mode != Fake::Stubborn {
                            exit = Some(0);
                        }
                        Ok(json!({}))
                    }
                    "restart" => Err((-32602, "only capture")),
                    _ => Err((-32601, "method not found")),
                };
                let reply = match answer {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": request["id"], "result": result}),
                    Err((code, message)) => json!({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}}),
                };
                socket.send(rpc(&reply)).await.unwrap();
                if let Some(code) = exit {
                    let _ = socket.close(None).await;
                    return code;
                }
            }
        }
    }
}

fn fake_agent(mode: Fake) {
    let (Ok(url), Ok(token)) = (
        std::env::var("FOLO_CAPTURE_LINK"),
        std::env::var("FOLO_CAPTURE_TOKEN"),
    ) else {
        return; // an ordinary test run
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    std::process::exit(runtime.block_on(agent(&url, &token, mode)));
}

#[test]
fn fake_agent_normal() {
    fake_agent(Fake::Normal);
}

#[test]
fn fake_agent_token() {
    fake_agent(Fake::Token);
}

#[test]
fn fake_agent_hang() {
    fake_agent(Fake::Hang);
}

#[test]
fn fake_agent_stubborn() {
    fake_agent(Fake::Stubborn);
}

#[test]
fn fake_agent_oversize() {
    fake_agent(Fake::Oversize);
}

fn rerun(name: &str) -> (PathBuf, Vec<std::ffi::OsString>) {
    (
        std::env::current_exe().unwrap(),
        [name, "--exact", "--nocapture", "--test-threads=1"]
            .map(Into::into)
            .to_vec(),
    )
}

fn engine_command() -> EngineCommand {
    let (program, args) = rerun("fake_engine_recording");
    EngineCommand { program, args }
}

/// The agent is `name` re-run; `folo-capture screens --json` is `listing`.
fn capture_command(name: &str, listing: &Path) -> CaptureCommand {
    let (program, run_args) = rerun(name);
    CaptureCommand {
        program,
        run_args,
        // The test harness prints to stdout, so the listing is a script's
        list_program: listing.to_path_buf(),
        list_args: Vec::new(),
    }
}

// ------------------------------------------------------------------ helpers

/// The node's logger is global to the process, so nodes in this process
/// run one at a time.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fresh home, removed afterwards, and the folder the fakes write to.
struct Home {
    path: PathBuf,
    port: u16,
}

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-capture-test-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        let port = StdListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let _ = std::fs::remove_dir_all(record_dir(port));
        std::fs::create_dir_all(record_dir(port)).unwrap();
        Self { path, port }
    }

    fn record(&self) -> PathBuf {
        record_dir(self.port)
    }

    /// A script that prints `{"screens": [...]}`, standing in for `folo-capture screens --json`.
    fn listing_script(&self, id: &str) -> PathBuf {
        let listing = format!(
            r#"{{"screens": [{{"id": "{id}", "name": "Listed", "width": 800, "height": 600, "primary": true}}]}}"#
        );
        self.write_listing_script(&listing)
    }

    #[cfg(windows)]
    fn write_listing_script(&self, listing: &str) -> PathBuf {
        let path = self.path.join("listing.cmd");
        std::fs::write(&path, format!("@echo {listing}\r\n")).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_listing_script(&self, listing: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = self.path.join("listing.sh");
        std::fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{listing}'\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn read_record(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.record().join(name)).ok()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        let _ = std::fs::remove_dir_all(record_dir(self.port));
    }
}

/// A node running on its own task.
struct Running {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<u8>,
}

impl Running {
    async fn start(home: &Home, agent: &str, service: bool) -> Self {
        let args = RunArgs {
            port: Some(home.port.to_string()),
            ..RunArgs::default()
        };
        let location = LocationArgs {
            home: Some(home.path.clone()),
            service,
        };
        let command = capture_command(agent, &home.listing_script("listed-0"));
        let (stop, stopped) = oneshot::channel::<()>();
        let done = tokio::spawn(async move {
            folo_node::node::run_with_workers(
                &location,
                &args,
                Some(engine_command()),
                Some(command),
                async {
                    let _ = stopped.await;
                },
            )
            .await
        });
        let node = Self {
            port: home.port,
            stop: Some(stop),
            done,
        };
        node.wait_for("the engine to link", |s| {
            s["workers"]["folo-source"]["state"] == "running"
        })
        .await;
        node
    }

    /// A request, or why it failed (the node isn't listening yet).
    async fn try_call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> std::io::Result<(u16, String)> {
        let port = self.port;
        let (method, path) = (method.to_owned(), path.to_owned());
        let body = body.map(|b| b.to_string());
        tokio::task::spawn_blocking(move || client::request(port, &method, &path, body.as_deref()))
            .await
            .unwrap()
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, String) {
        self.try_call(method, path, body).await.unwrap()
    }

    async fn json(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let (status, text) = self.call(method, path, body).await;
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn status(&self) -> Option<Value> {
        match self.try_call("GET", "/api/v1/system/status", None).await {
            Ok((200, text)) => serde_json::from_str(&text).ok(),
            _ => None,
        }
    }

    /// Waits until the node's status satisfies `wanted`.
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

    async fn capture_state(&self) -> String {
        let status = self.status().await.unwrap();
        status["workers"]["folo-capture"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// Makes capture wanted: a canvas surface `wall` (64 x 36) with a Screen
    /// layer, the local source rendering it, and an enabled screen.
    async fn want_capture(&self) -> String {
        let zone = json!({"name": "Wall", "surfaces": [
            {"id": "wall", "kind": "canvas", "resolution": {"width": 64, "height": 36}, "role": "canvas.room"},
        ]});
        let (_, zone) = self.json("POST", "/api/v1/zones", Some(zone)).await;
        let zone_id = zone["id"].as_str().unwrap().to_owned();
        self.set_layer(&zone_id, true).await;
        let (status, _) = self
            .json("PUT", "/api/v1/sources/local", Some(Self::source(false)))
            .await;
        assert_eq!(status, 200);
        zone_id
    }

    fn source(game: bool) -> Value {
        json!({"surfaces": ["wall"], "inputs": {"screens": [
            {"id": "fake-0", "enabled": true, "profile": if game { "game" } else { "video" }},
        ]}})
    }

    async fn set_layer(&self, zone_id: &str, enabled: bool) {
        let layer = json!({"id": "screen1", "effect": {"id": "screen"}, "enabled": enabled});
        let show = json!({
            "active": {zone_id: "s1"},
            "scenes": [{"id": "s1", "name": "Screen", "layers_by_role": {"canvas.room": [layer]}}],
        });
        assert_eq!(self.json("PUT", "/api/v1/show", Some(show)).await.0, 200);
    }

    async fn stop(mut self) -> u8 {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(30), self.done)
            .await
            .expect("the node stops")
            .unwrap()
    }
}

/// A live channel connection that keeps everything it is sent.
struct Live {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    texts: Vec<String>,
    binary: Vec<Vec<u8>>,
}

impl Live {
    async fn connect(port: u16, topics: &[&str]) -> Self {
        let url = format!("ws://127.0.0.1:{port}/api/v1/live");
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let subscribe = json!({"subscribe": topics});
        socket
            .send(Message::Text(subscribe.to_string().into()))
            .await
            .unwrap();
        Self {
            socket,
            texts: Vec::new(),
            binary: Vec::new(),
        }
    }

    /// Reads until the local source's `screen` satisfies `wanted`.
    async fn screen_where(&mut self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = tokio::time::timeout(left, self.socket.next())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "timed out waiting for {what}; last: {:?}",
                        self.texts.last()
                    )
                })
                .expect("the live channel stays open")
                .unwrap();
            match message {
                Message::Text(text) => {
                    self.texts.push(text.as_str().to_owned());
                    let message: Value = serde_json::from_str(text.as_str()).unwrap();
                    let screen = &message["data"]["sources"]["local"]["screen"];
                    if message["type"] == "status" && screen.is_object() && wanted(screen) {
                        return screen.clone();
                    }
                }
                Message::Binary(bytes) => self.binary.push(bytes.to_vec()),
                _ => {}
            }
        }
    }

    /// Reads whatever comes for `time`.
    async fn collect_for(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while let Ok(Some(Ok(message))) = tokio::time::timeout(
            end.saturating_duration_since(Instant::now()),
            self.socket.next(),
        )
        .await
        {
            match message {
                Message::Text(text) => self.texts.push(text.as_str().to_owned()),
                Message::Binary(bytes) => self.binary.push(bytes.to_vec()),
                _ => {}
            }
        }
    }
}

async fn wait_until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn starts(home: &Home) -> Vec<Value> {
    String::from_utf8(home.read_record("agent-starts.jsonl").unwrap_or_default())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn events(home: &Home) -> Vec<String> {
    String::from_utf8(home.read_record("agent-events").unwrap_or_default())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn every_file(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            every_file(&path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((path, bytes));
        }
    }
}

/// A WebSocket client for the capture link that is no agent of the node.
async fn rogue(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/api/v1/capture-link");
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "folo.capture.v1".parse().unwrap());
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

fn hello(token: &str) -> Message {
    let hello =
        json!({"version": ["1"], "agent": {"version": "0", "platform": "test"}, "token": token});
    Message::Text(hello.to_string().into())
}

// ------------------------------------------------------------------ the tests

#[tokio::test(flavor = "multi_thread")]
async fn capture_runs_while_it_is_wanted_and_the_agent_is_stopped_when_it_isnt() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_normal", false).await;
    let mut live = Live::connect(node.port, &["input:local"]).await;

    // Nothing wanted: no agent, and the screen is off
    let off = live
        .screen_where("an off screen", |s| s["state"] == "off")
        .await;
    assert_eq!(off["backend"], "");
    assert_eq!(node.capture_state().await, "stopped");
    assert!(
        home.read_record("agent-token").is_none(),
        "no agent was started"
    );

    // Wanted: the node starts the agent, which links and is told what to do
    let zone_id = node.want_capture().await;
    let capturing = live
        .screen_where("capturing", |s| s["state"] == "capturing")
        .await;
    assert_eq!(capturing["backend"], "fake");
    assert_eq!(capturing["screen"], "fake-0");
    assert_eq!(capturing["bars"]["top"], 0.1);
    let status = node.status().await.unwrap();
    assert_eq!(status["workers"]["folo-capture"]["state"], "running");
    assert!(status["workers"]["folo-capture"]["pid"].as_u64().unwrap() > 0);
    let first = starts(&home);
    assert_eq!(first.len(), 1, "{first:?}");
    // A 64 x 36 canvas: frames fit twice that, at 60 Hz
    assert_eq!(first[0]["screen"], "fake-0");
    assert_eq!(first[0]["width"], 128);
    assert_eq!(first[0]["height"], 72);
    assert_eq!(first[0]["rate_hz"], 60);
    assert_eq!(first[0]["profile"], "video");
    assert_eq!(first[0]["black_bars"]["preset"], "auto");
    assert!(first[0].get("restore_token").is_none());

    // A changed wish reaches the agent in place
    let (status, _) = node
        .json("PUT", "/api/v1/sources/local", Some(Running::source(true)))
        .await;
    assert_eq!(status, 200);
    wait_until("the second capture.start", || starts(&home).len() == 2).await;
    assert_eq!(starts(&home)[1]["profile"], "game");
    assert_eq!(
        node.status().await.unwrap()["workers"]["folo-capture"]["restarts"],
        0
    );

    // No longer wanted: capture.stop, then restart, and the agent ends
    node.set_layer(&zone_id, false).await;
    live.screen_where("off again", |s| s["state"] == "off")
        .await;
    node.wait_for("the agent to end", |s| {
        s["workers"]["folo-capture"]["state"] == "stopped"
    })
    .await;
    assert_eq!(events(&home), ["stop", "restart"]);
    // The engine goes on
    assert_eq!(
        node.status().await.unwrap()["workers"]["folo-source"]["state"],
        "running"
    );

    // Wanted again: a new agent, with a new token
    let old_token = home.read_record("agent-token").unwrap();
    node.set_layer(&zone_id, true).await;
    live.screen_where("capturing again", |s| s["state"] == "capturing")
        .await;
    assert_ne!(home.read_record("agent-token").unwrap(), old_token);
    assert_eq!(starts(&home).len(), 3);
    assert_eq!(
        node.status().await.unwrap()["workers"]["folo-capture"]["restarts"],
        0
    );

    assert_eq!(node.stop().await, 0);
    assert_eq!(folo_node::lock::holder(&home.path.join("folo.lock")), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_frames_reach_the_engine_unchanged() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_normal", false).await;
    node.want_capture().await;
    wait_until("frames at the engine", || {
        home.read_record("engine-frames")
            .and_then(|n| String::from_utf8(n).ok())
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| n >= 20)
    })
    .await;
    let latest = home.read_record("engine-latest.bin").unwrap();
    assert_eq!(latest[0], 0x04);
    let frame = codec::decode(&latest[1..]).expect("a frame the codec reads");
    assert_eq!(
        latest,
        frame_message(frame.sequence),
        "byte for byte what the agent sent"
    );
    assert!(frame.sequence >= 10);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_restore_token_round_trips_through_secrets_and_never_reaches_clients() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    std::fs::write(
        home.path.join("secrets.json"),
        r#"{"capture": {"restore_token": "stored-token"}}"#,
    )
    .unwrap();
    let node = Running::start(&home, "fake_agent_token", false).await;
    let mut live = Live::connect(node.port, &["input:local"]).await;
    let zone_id = node.want_capture().await;

    // The stored token goes with the first start from off
    live.screen_where("capturing", |s| s["state"] == "capturing")
        .await;
    assert_eq!(starts(&home)[0]["restore_token"], "stored-token");
    // The new one the agent reports is stored, owner-only
    let secrets_path = home.path.join("secrets.json");
    wait_until("the new token in secrets.json", || {
        std::fs::read_to_string(&secrets_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|secrets| secrets["capture"]["restore_token"] == "fresh-token")
    })
    .await;
    let secrets: Value = serde_json::from_slice(&std::fs::read(&secrets_path).unwrap()).unwrap();
    assert!(
        secrets["identity"]["private_key"]
            .as_str()
            .is_some_and(|k| !k.is_empty()),
        "the identity stays"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&secrets_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "secrets.json is owner-only: {mode:o}");
    }

    // An in-place change doesn't send it again; a start from off sends the new one
    node.json("PUT", "/api/v1/sources/local", Some(Running::source(true)))
        .await;
    wait_until("the second start", || starts(&home).len() == 2).await;
    assert!(starts(&home)[1].get("restore_token").is_none());
    node.set_layer(&zone_id, false).await;
    node.wait_for("the agent to end", |s| {
        s["workers"]["folo-capture"]["state"] == "stopped"
    })
    .await;
    node.set_layer(&zone_id, true).await;
    wait_until("the third start", || starts(&home).len() == 3).await;
    assert_eq!(starts(&home)[2]["restore_token"], "fresh-token");

    // Clients never see a token: the live channel, the status, the API
    live.collect_for(Duration::from_secs(1)).await;
    let mut shown: Vec<String> = live.texts.clone();
    for path in [
        "/api/v1/system/status",
        "/api/v1/state",
        "/api/v1/sources/local",
        "/api/v1/sources/local/inputs",
        "/api/v1/system/logs?lines=10000",
    ] {
        shown.push(node.call("GET", path, None).await.1);
    }
    for text in &shown {
        for token in ["stored-token", "fresh-token", "restore_token"] {
            assert!(!text.contains(token), "{token} in {text}");
        }
    }
    let log = std::fs::read_to_string(home.path.join("logs").join("folo.log")).unwrap();
    for token in ["stored-token", "fresh-token"] {
        assert!(!log.contains(token), "{token} in the log");
    }
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_capture_link_refuses_what_is_no_agent_of_the_node() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_normal", false).await;
    node.want_capture().await;
    let mut live = Live::connect(node.port, &["input:local"]).await;
    live.screen_where("capturing", |s| s["state"] == "capturing")
        .await;

    // A wrong token is rejected, and gets nothing else
    let mut socket = rogue(node.port).await;
    socket.send(hello("guess")).await.unwrap();
    let Some(Ok(Message::Text(answer))) = socket.next().await else {
        panic!("no answer to a wrong token");
    };
    assert_eq!(
        serde_json::from_str::<Value>(answer.as_str()).unwrap(),
        json!({"version": "1", "result": "rejected"})
    );
    assert!(matches!(
        socket.next().await,
        None | Some(Err(_) | Ok(Message::Close(_)))
    ));

    // The right token, but an agent is linked: closed without a welcome
    let token = String::from_utf8(home.read_record("agent-token").unwrap()).unwrap();
    let mut second = rogue(node.port).await;
    second.send(hello(&token)).await.unwrap();
    match second.next().await {
        None | Some(Err(_) | Ok(Message::Close(_))) => {}
        Some(Ok(other)) => panic!("a second agent got an answer: {other:?}"),
    }
    // And the linked agent was not disturbed: a new page sees it capturing
    let mut again = Live::connect(node.port, &["input:local"]).await;
    again
        .screen_where("still capturing", |s| s["state"] == "capturing")
        .await;
    assert_eq!(
        node.status().await.unwrap()["workers"]["folo-capture"]["restarts"],
        0
    );

    // A connection that never says hello is closed after the handshake time
    let mut silent = rogue(node.port).await;
    let began = Instant::now();
    let ended = tokio::time::timeout(Duration::from_secs(10), silent.next())
        .await
        .expect("the node closes a silent connection");
    assert!(matches!(ended, None | Some(Err(_) | Ok(Message::Close(_)))));
    assert!(
        began.elapsed() >= Duration::from_secs(4),
        "{:?}",
        began.elapsed()
    );

    // A plain request is told to upgrade
    let (status, body) = node.json("GET", "/api/v1/capture-link", None).await;
    assert_eq!(
        (status, &body["error"]["code"]),
        (400, &json!("websocket_required"))
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_over_the_limit_ends_the_link() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_oversize", false).await;
    node.want_capture().await;
    wait_until("the node to close the link", || {
        home.read_record("oversize-closed").is_some()
    })
    .await;
    assert!(
        home.read_record("engine-frames").is_none(),
        "nothing was forwarded"
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hung_agent_is_killed_and_started_again() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_hang", false).await;
    node.want_capture().await;
    let began = Instant::now();
    node.wait_for("a restart of the hung agent", |s| {
        s["workers"]["folo-capture"]["restarts"].as_u64() >= Some(1)
    })
    .await;
    // Five seconds of silence, not before
    assert!(
        began.elapsed() >= Duration::from_secs(4),
        "{:?}",
        began.elapsed()
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_ignores_restart_is_killed_after_five_seconds() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_stubborn", false).await;
    let zone_id = node.want_capture().await;
    node.wait_for("the agent to link", |s| {
        s["workers"]["folo-capture"]["state"] == "running"
    })
    .await;
    wait_until("the capture to start", || !starts(&home).is_empty()).await;
    let pid = String::from_utf8(home.read_record("agent-pid").unwrap()).unwrap();
    node.set_layer(&zone_id, false).await;
    let began = Instant::now();
    wait_until("the restart request", || {
        events(&home).contains(&"restart".to_owned())
    })
    .await;
    // It answered and stayed
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(node.capture_state().await, "running");
    node.wait_for("the agent to be killed", |s| {
        s["workers"]["folo-capture"]["state"] == "stopped"
    })
    .await;
    let waited = began.elapsed();
    assert!(waited >= Duration::from_secs(4), "{waited:?}");
    assert!(waited < Duration::from_secs(10), "{waited:?}");
    if cfg!(target_os = "linux") {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "process {pid} is still there"
        );
    }
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn screens_come_from_the_linked_agent_or_else_from_the_listing_command() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_normal", false).await;
    // No agent linked: `folo-capture screens --json`
    let (status, inputs) = node.json("GET", "/api/v1/sources/local/inputs", None).await;
    assert_eq!(status, 200);
    assert_eq!(inputs["screens"][0]["id"], "listed-0", "{inputs}");
    assert_eq!(inputs["media"], true, "the rest of the engine's list stays");
    // A linked agent answers screens.list itself
    let mut live = Live::connect(node.port, &["input:local"]).await;
    node.want_capture().await;
    live.screen_where("capturing", |s| s["state"] == "capturing")
        .await;
    let (_, inputs) = node.json("GET", "/api/v1/sources/local/inputs", None).await;
    assert_eq!(inputs["screens"][0]["id"], "fake-0", "{inputs}");
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn as_a_service_no_agent_runs_and_capture_is_unavailable() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_normal", true).await;
    let mut live = Live::connect(node.port, &["input:local"]).await;
    live.screen_where("off", |s| s["state"] == "off").await;
    node.want_capture().await;
    let unavailable = live
        .screen_where("unavailable", |s| s["state"] == "unavailable")
        .await;
    assert!(
        unavailable["message"]
            .as_str()
            .is_some_and(|m| m.contains("service"))
    );
    let (_, inputs) = node.json("GET", "/api/v1/sources/local/inputs", None).await;
    assert_eq!(inputs["screens"], json!([]));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(node.capture_state().await, "stopped");
    assert!(
        home.read_record("agent-token").is_none(),
        "no agent was started"
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_frames_are_in_no_log_status_answer_message_or_file_of_the_node() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, "fake_agent_normal", false).await;
    let mut live = Live::connect(
        node.port,
        &[
            "status",
            "input:local",
            "beat:local",
            "media:local",
            "spectrum:local",
            "levels:wall",
            "zone:default",
        ],
    )
    .await;
    node.want_capture().await;
    live.screen_where("capturing", |s| s["state"] == "capturing")
        .await;
    wait_until("frames at the engine", || {
        home.read_record("engine-frames")
            .and_then(|n| String::from_utf8(n).ok())
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| n >= 40)
    })
    .await;
    live.collect_for(Duration::from_secs(1)).await;

    // Sanity: the engine really got marker frames, so a leak would be seen
    let reached_the_engine = home.read_record("engine-latest.bin").unwrap();
    assert!(contains(&reached_the_engine, &marker_run()));

    // Everything the node shows or keeps
    let mut seen: Vec<(String, Vec<u8>)> = Vec::new();
    for path in [
        "/api/v1/health",
        "/api/v1/info",
        "/api/v1/state",
        "/api/v1/system",
        "/api/v1/system/status",
        "/api/v1/system/logs?lines=10000",
        "/api/v1/zones",
        "/api/v1/show",
        "/api/v1/sources",
        "/api/v1/sources/local",
        "/api/v1/sources/local/inputs",
        "/api/v1/sources/local/media",
        "/api/v1/effects",
        "/api/v1/providers",
    ] {
        seen.push((
            path.to_owned(),
            node.call("GET", path, None).await.1.into_bytes(),
        ));
    }
    for (index, text) in live.texts.iter().enumerate() {
        seen.push((format!("live message {index}"), text.clone().into_bytes()));
    }
    for (index, bytes) in live.binary.iter().enumerate() {
        seen.push((format!("live frame {index}"), bytes.clone()));
    }
    let mut files = Vec::new();
    every_file(&home.path, &mut files);
    assert!(files.iter().any(|(p, _)| p.ends_with("folo.log")));
    assert!(files.iter().any(|(p, _)| p.ends_with("secrets.json")));
    for (path, bytes) in files {
        seen.push((path.display().to_string(), bytes));
    }
    let token = home.read_record("agent-token").unwrap();
    let one_frame = frame_message(1);
    let payload = &one_frame[1 + 36..];
    for (what, bytes) in &seen {
        assert!(!contains(bytes, &marker_run()), "marker pixels in {what}");
        assert!(!contains(bytes, &payload[3..]), "frame pixels in {what}");
        assert!(
            !contains(bytes, &token),
            "the agent's launch token in {what}"
        );
        assert!(!contains(bytes, b"FOLC"), "a frame header in {what}");
    }
    // The topics that could carry a picture carried none of these
    assert!(live.binary.iter().all(|b| b[0] != 0x04));
    node.stop().await;
}
