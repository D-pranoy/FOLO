//! The client API and the live channel end to end, in this process (RB-07):
//! the documents, the effect catalog, requests to the engine, and the live
//! channel streaming what the engine sends.
//!
//! The fake engine is this test binary run again by the node: the test
//! `fake_engine_streaming` acts as an engine when the node's link variables
//! are set, and does nothing otherwise. It publishes what `config.apply`
//! asks for and answers the requests the API forwards.

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::TcpListener as StdListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use folo_node::cli::{LocationArgs, RunArgs};
use folo_protocol::generated::{
    EffectInfo, Info, InputList, LiveLevelsMessage, LiveStatusMessage, LogLines, MediaState, State,
    SystemSettings, Zone, ZoneActivation,
};
use folo_supervisor::EngineCommand;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

// ------------------------------------------------------------------ the fake engine

/// The album art the fake engine shows: art id 3.
const ART: &[u8] = b"\x89PNG not really";
const ART_ID: u64 = 3;
/// Values in the fake spectrum.
const BANDS: u16 = 192;

fn rpc(value: &Value) -> Message {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(value.to_string().as_bytes());
    Message::Binary(bytes.into())
}

/// A surface frame (type byte included) of `width` cells, all `value`.
fn surface_frame(index: u16, width: u16, sequence: u32, value: u8) -> Message {
    let mut bytes = vec![0x02];
    bytes.extend_from_slice(b"FOLS");
    bytes.extend_from_slice(&[1, 1, 2, 0]);
    bytes.extend_from_slice(&index.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend(std::iter::repeat_n(value, usize::from(width) * 3));
    Message::Binary(bytes.into())
}

/// A signal frame (type byte included).
fn signal_frame(index: u16, values: &[f32], sequence: u32) -> Message {
    let mut bytes = vec![0x03];
    bytes.extend_from_slice(b"FOLG");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&index.to_le_bytes());
    let count = u16::try_from(values.len()).unwrap();
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Message::Binary(bytes.into())
}

fn input_status() -> Value {
    json!({
        "state": "ok", "label": "Speakers", "message": "", "rate": 48000, "channels": 2,
        "event_driven": true, "packets_per_s": 100.0, "delay_ms": 2.0, "glitches": 0,
        "peaks_db": [-12.0, -13.0], "clip": false, "gain_db": 0.0, "auto_gain_db": 0.0,
    })
}

fn media_state() -> Value {
    json!({
        "state": "playing", "app": "Player.exe", "title": "Song", "artist": "Artist",
        "album": "Album", "art": ART_ID, "colors": ["#4B00FF"],
    })
}

/// What the engine was last asked to publish, as the fake reports it in
/// `inputs.list`'s `default_output`: `signals:beat,spectrum levels:0`.
fn asked(publish: &Value) -> String {
    let names: Vec<&str> = publish["signals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["name"].as_str())
        .collect();
    let levels: Vec<String> = publish["levels"]
        .as_array()
        .into_iter()
        .flatten()
        .map(ToString::to_string)
        .collect();
    format!("signals:{} levels:{}", names.join(","), levels.join(","))
}

fn answer(request: &Value, publish: &Value) -> Value {
    let id = &request["id"];
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
    let refused = |code: i64, message: &str| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
    match request["method"].as_str().unwrap_or_default() {
        "inputs.list" => ok(json!({
            "default_output": asked(publish), "outputs": [], "inputs": [], "apps": [],
            "screens": [], "media": true,
        })),
        "media.art" if request["params"]["art_id"] == ART_ID => ok(json!({
            "content_type": "image/png", "data": STANDARD.encode(ART),
        })),
        "media.control" if request["params"]["action"] == "next" => ok(json!({})),
        "media.art" | "media.control" => refused(-32000, "not available"),
        "config.apply" | "restart" => ok(json!({})),
        _ => refused(-32601, "method not found"),
    }
}

async fn engine(url: &str, token: &str) -> i32 {
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
    let mut publish = json!({"surfaces": [], "signals": [], "levels": []});
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let mut sequence = 0u32;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                sequence = sequence.wrapping_add(1);
                let status = json!({"state": "running", "workers": {}, "inputs": {}, "errors": []});
                let mut out = vec![
                    rpc(&json!({"jsonrpc": "2.0", "method": "status", "params": status})),
                    rpc(&json!({"jsonrpc": "2.0", "method": "input", "params": input_status()})),
                    rpc(&json!({"jsonrpc": "2.0", "method": "media", "params": media_state()})),
                ];
                for surface in publish["surfaces"].as_array().into_iter().flatten() {
                    let index = u16::try_from(surface["index"].as_u64().unwrap()).unwrap();
                    let width = u16::try_from(surface["width"].as_u64().unwrap()).unwrap();
                    #[allow(clippy::cast_possible_truncation)]
                    let value = sequence as u8;
                    out.push(surface_frame(index, width, sequence, value));
                }
                for signal in publish["signals"].as_array().into_iter().flatten() {
                    let index = u16::try_from(signal["index"].as_u64().unwrap()).unwrap();
                    let values = match signal["name"].as_str() {
                        Some("beat") => vec![120.0, 0.7, 1.0, 0.25, 1.0],
                        _ => vec![-60.0; usize::from(BANDS)],
                    };
                    out.push(signal_frame(index, &values, sequence));
                }
                for surface in publish["levels"].as_array().into_iter().flatten() {
                    let params = json!({"surface": surface, "layers": {"l1": [-30.0, 0.5, -33.0]}});
                    out.push(rpc(&json!({"jsonrpc": "2.0", "method": "levels", "params": params})));
                }
                for message in out {
                    if socket.send(message).await.is_err() {
                        return 1;
                    }
                }
            }
            message = socket.next() => {
                let Some(Ok(Message::Binary(bytes))) = message else {
                    return 1;
                };
                let request: Value = serde_json::from_slice(&bytes[1..]).unwrap();
                if request["method"] == "config.apply" {
                    publish = request["params"]["publish"].clone();
                }
                socket.send(rpc(&answer(&request, &publish))).await.unwrap();
                if request["method"] == "restart" {
                    let _ = socket.close(None).await;
                    return 0;
                }
            }
        }
    }
}

#[test]
fn fake_engine_streaming() {
    let (Ok(url), Ok(token)) = (
        std::env::var("FOLO_SOURCE_LINK"),
        std::env::var("FOLO_SOURCE_TOKEN"),
    ) else {
        return; // an ordinary test run
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let status = runtime.block_on(engine(&url, &token));
    std::process::exit(status);
}

// ------------------------------------------------------------------ helpers

/// The node's logger is global to the process, so nodes in this process
/// run one at a time.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fresh home, removed afterwards.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-api-test-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
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

/// A node running on its own task, with the fake engine or none.
struct Running {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<u8>,
}

impl Running {
    async fn start(home: &Home, engine: bool) -> Self {
        let port = free_port();
        let args = RunArgs {
            port: Some(port.to_string()),
            local_source: (!engine).then(|| "off".to_owned()),
            ..RunArgs::default()
        };
        let location = LocationArgs {
            home: Some(home.0.clone()),
            service: false,
        };
        let command = EngineCommand {
            program: std::env::current_exe().unwrap(),
            args: [
                "fake_engine_streaming",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ]
            .map(Into::into)
            .to_vec(),
        };
        let (stop, stopped) = oneshot::channel::<()>();
        let done = tokio::spawn(async move {
            folo_node::node::run_with(&location, &args, Some(command), async {
                let _ = stopped.await;
            })
            .await
        });
        let node = Self {
            port,
            stop: Some(stop),
            done,
        };
        node.wait_for("the node", |_| true).await;
        node
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Answer {
        let raw = request_text(self.port, method, path, headers, body);
        let port = self.port;
        tokio::task::spawn_blocking(move || send(port, &raw))
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> Answer {
        self.call("GET", path, &[], None).await
    }

    /// A JSON request with a body.
    async fn send_json(&self, method: &str, path: &str, body: &Value) -> Answer {
        let json = [("Content-Type", "application/json")];
        self.call(method, path, &json, Some(&body.to_string()))
            .await
    }

    async fn wait_for(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let raw = request_text(self.port, "GET", "/api/v1/system/status", &[], None);
            let port = self.port;
            let answer = tokio::task::spawn_blocking(move || try_send(port, &raw))
                .await
                .unwrap();
            if let Ok(answer) = answer
                && answer.status == 200
                && wanted(&answer.json())
            {
                return answer.json();
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

/// An HTTP answer.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn code(&self) -> Value {
        self.json()["error"]["code"].clone()
    }
}

fn request_text(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Vec<u8> {
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    let body = body.unwrap_or_default();
    if !body.is_empty() || matches!(method, "POST" | "PUT") {
        let _ = write!(head, "Content-Length: {}\r\n", body.len());
    }
    head.push_str("\r\n");
    let mut raw = head.into_bytes();
    raw.extend_from_slice(body.as_bytes());
    raw
}

fn send(port: u16, raw: &[u8]) -> Answer {
    try_send(port, raw).unwrap()
}

/// Sends a request, or fails when the node can't be reached: Linux refuses a
/// connection at once while the node isn't listening yet.
fn try_send(port: u16, raw: &[u8]) -> std::io::Result<Answer> {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
    stream.write_all(raw)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(response[..split].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let chunked = headers
        .iter()
        .any(|(n, v)| n.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let mut body = response[split + 4..].to_vec();
    if chunked {
        body = dechunk(&body);
    }
    Ok(Answer {
        status,
        headers,
        body,
    })
}

fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let end = body.windows(2).position(|w| w == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&body[..end]).unwrap().trim(), 16).unwrap();
        body = &body[end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

/// Where the live test keeps what the node sent, one message per file, for
/// the conformance harness: `scripts/conformance.py messages target/tmp/live-messages`.
fn captured() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live-messages")
}

/// Keeps a message the node sent, named after its message in `specs/asyncapi.yaml`.
fn capture(name: &str, bytes: &[u8]) {
    std::fs::write(captured().join(name), bytes).unwrap();
}

/// Checks that `value` is a `T` of the generated contract types.
fn conforms<T: serde::de::DeserializeOwned>(value: &Value, what: &str) {
    if let Err(error) = serde_json::from_value::<T>(value.clone()) {
        panic!("{what} doesn't conform: {error}\n{value}");
    }
}

// ------------------------------------------------------------------ the tests

#[tokio::test(flavor = "multi_thread")]
async fn the_api_serves_the_node_and_its_documents() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, false).await;
    the_node_and_its_catalog(&node).await;
    zones_with_revisions(&node).await;
    the_show_and_bodies(&node).await;
    providers_and_full_lists(&node).await;
    the_rest(&node).await;
    assert_eq!(node.stop().await, 0);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(home.0.join("zones.json")).unwrap()).unwrap();
    assert_eq!(
        saved["zones"][0]["name"], "Desk lamp",
        "written when the node stopped"
    );
}

async fn the_node_and_its_catalog(node: &Running) {
    let health = node.get("/api/v1/health").await;
    assert_eq!(health.json(), json!({"status": "ok"}));
    assert_eq!(health.header("cache-control"), Some("no-store"));
    assert_eq!(health.header("x-content-type-options"), Some("nosniff"));

    let info = node.get("/api/v1/info").await.json();
    conforms::<Info>(&info, "/info");
    assert_eq!(
        info["contracts"],
        json!({"client_api": "1.3.0", "source_link": "1.3", "config": 2})
    );

    let effects = node.get("/api/v1/effects").await.json();
    conforms::<Vec<EffectInfo>>(&effects, "/effects");
    assert_eq!(effects.as_array().unwrap().len(), 6);
    let state = node.get("/api/v1/state").await.json();
    conforms::<State>(&state, "/state");
    conforms::<SystemSettings>(&node.get("/api/v1/system").await.json(), "/system");
}

/// Zones: add, read, replace with and without a matching revision.
async fn zones_with_revisions(node: &Running) {
    let zone = json!({"name": "Desk", "surfaces": [{"id": "main", "kind": "strip", "resolution": {"width": 10, "height": 1}}]});
    let created = node.send_json("POST", "/api/v1/zones", &zone).await;
    assert_eq!(created.status, 201);
    let created = created.json();
    conforms::<Zone>(&created, "a created zone");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 8, "a new id");
    let again = node
        .send_json(
            "POST",
            "/api/v1/zones",
            &json!({"id": id, "name": "Same id"}),
        )
        .await
        .json();
    assert_ne!(again["id"], id, "a taken id gets a new one");
    let path = format!("/api/v1/zones/{id}");
    assert_eq!(node.get(&path).await.json()["name"], "Desk");
    let revision = node.get("/api/v1/zones").await.json()["revision"]
        .as_u64()
        .unwrap();
    let json_type = ("Content-Type", "application/json");
    let older = (revision - 1).to_string();
    let renamed = json!({"name": "Desk lamp", "id": "ignored"}).to_string();
    let refused = node
        .call(
            "PUT",
            &path,
            &[json_type, ("If-Match", &older)],
            Some(&renamed),
        )
        .await;
    assert_eq!(refused.status, 412);
    assert_eq!(refused.code(), "revision_mismatch");
    assert_eq!(refused.json()["error"]["details"]["revision"], revision);
    let current = format!("\"{revision}\"");
    let replaced = node
        .call(
            "PUT",
            &path,
            &[json_type, ("If-Match", &current)],
            Some(&renamed),
        )
        .await;
    assert_eq!(replaced.status, 200);
    assert_eq!(replaced.json()["name"], "Desk lamp");
    assert_eq!(replaced.json()["id"], id, "the path's id");
    assert_eq!(
        node.get("/api/v1/zones").await.json()["revision"],
        revision + 1
    );
    let missing = node
        .send_json("PUT", "/api/v1/zones/nope", &json!({}))
        .await;
    assert_eq!(missing.status, 404);
    // A body that isn't an item is refused, and the item stays
    let not_an_item = node.send_json("PUT", &path, &json!([])).await;
    assert_eq!(not_an_item.status, 422);
    assert_eq!(not_an_item.code(), "structural_error");
    assert_eq!(node.get(&path).await.json()["name"], "Desk lamp");
}

/// The show: a whole-document write, clamped; structural errors, bodies
/// that aren't JSON and bodies over the limit refused.
async fn the_show_and_bodies(node: &Running) {
    let json_type = ("Content-Type", "application/json");
    let show = json!({"scenes": [{"id": "s1", "name": "Scene", "tags": ["x"]}], "unknown": 1});
    let stored = node.send_json("PUT", "/api/v1/show", &show).await.json();
    assert_eq!(stored["scenes"][0]["id"], "s1");
    assert!(stored.get("unknown").is_none(), "clamped");
    let structural = node.send_json("PUT", "/api/v1/show", &json!([])).await;
    assert_eq!(structural.status, 422);
    assert_eq!(structural.code(), "structural_error");
    let not_json = node
        .call("PUT", "/api/v1/show", &[json_type], Some("{"))
        .await;
    assert_eq!(not_json.status, 400);
    assert_eq!(not_json.code(), "bad_body");
    let huge = format!("{{\"x\": \"{}\"}}", "a".repeat(1024 * 1024));
    let too_large = node
        .call("PUT", "/api/v1/show", &[json_type], Some(&huge))
        .await;
    assert_eq!(too_large.status, 413);
    assert_eq!(too_large.code(), "body_too_large");
}

/// Providers are documents too; a full list refuses another item rather
/// than dropping it.
async fn providers_and_full_lists(node: &Running) {
    let json_type = ("Content-Type", "application/json");
    let provider = node
        .send_json(
            "POST",
            "/api/v1/providers",
            &json!({"kind": "wled", "name": "Shelf"}),
        )
        .await;
    assert_eq!(provider.status, 201);
    let provider_path = format!(
        "/api/v1/providers/{}",
        provider.json()["id"].as_str().unwrap()
    );
    let removed = node
        .call("DELETE", &provider_path, &[json_type], None)
        .await;
    assert_eq!(removed.status, 204);
    let gone = node
        .call("DELETE", &provider_path, &[json_type], None)
        .await;
    assert_eq!(gone.status, 404);
    for _ in 0..64 {
        let added = node
            .send_json("POST", "/api/v1/providers", &json!({}))
            .await;
        assert_eq!(added.status, 201);
    }
    let full = node
        .send_json("POST", "/api/v1/providers", &json!({}))
        .await;
    assert_eq!((full.status, full.code()), (409, json!("list_full")));
}

/// Without a local engine there's no source `local`; later subsystems, the
/// log, the UI's page and the live channel without an upgrade.
async fn the_rest(node: &Running) {
    assert_eq!(node.get("/api/v1/sources/local").await.status, 404);
    assert_eq!(node.get("/api/v1/sources/local/media").await.status, 404);

    let later = node
        .send_json("POST", "/api/v1/sources/invites", &json!({}))
        .await;
    assert_eq!(later.status, 501);
    assert_eq!(later.json()["error"]["details"]["runbook"], "RB-16");
    let logs = node.get("/api/v1/system/logs?lines=3").await.json();
    conforms::<LogLines>(&logs, "/system/logs");
    assert_eq!(logs["lines"].as_array().unwrap().len(), 3);
    // The logger is the process's: the other test's engine may write too
    let lines = logs["lines"].as_array().unwrap();
    assert!(lines.iter().any(|line| line["source"] == "node"), "{logs}");
    assert_eq!(node.get("/api/v1/system/logs?lines=0").await.status, 400);
    let page = node.get("/").await;
    assert_eq!(page.status, 200);
    assert!(
        page.header("content-security-policy")
            .unwrap()
            .contains("script-src 'self'")
    );
    assert_eq!(page.header("cache-control"), Some("no-store"));
    assert_eq!(node.get("/api/v1/live").await.code(), "websocket_required");
}

/// Sets up a zone with the surface `main`, a scene on it, and the local
/// source rendering it.
async fn assign_main(node: &Running) -> String {
    let zone = json!({"name": "Desk", "surfaces": [
        {"id": "main", "kind": "strip", "resolution": {"width": 10, "height": 1}, "role": "strip.main"},
    ]});
    let zone = node.send_json("POST", "/api/v1/zones", &zone).await.json();
    let zone_id = zone["id"].as_str().unwrap().to_owned();
    let show = json!({
        "active": {zone_id.clone(): "s1"},
        "scenes": [{"id": "s1", "name": "Scene", "layers_by_role": {"strip.main": [{"id": "l1"}]}}],
    });
    assert_eq!(
        node.send_json("PUT", "/api/v1/show", &show).await.status,
        200
    );
    let local = node
        .send_json(
            "PUT",
            "/api/v1/sources/local",
            &json!({"surfaces": ["main"]}),
        )
        .await;
    assert_eq!(local.status, 200, "the local source is added");
    zone_id
}

/// A live channel connection.
type Live = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// The next live message, within a few seconds.
async fn next(socket: &mut Live) -> Message {
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("a live message in time")
        .expect("the connection is open")
        .expect("a message")
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_reach_the_engine_and_the_live_channel_streams_it() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, true).await;
    let zone_id = assign_main(&node).await;
    node.wait_for("the engine to link", |s| {
        s["workers"]["folo-source"]["state"] == "running"
    })
    .await;
    requests_reach_the_source(&node).await;

    let url = format!("ws://127.0.0.1:{}/api/v1/live", node.port);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    every_kind_of_data_arrives(&mut socket, &zone_id).await;
    // The engine is asked for what the clients watch
    let inputs = node.get("/api/v1/sources/local/inputs").await.json();
    assert_eq!(inputs["default_output"], "signals:beat,spectrum levels:0");
    documents_written_are_announced(&node, &mut socket).await;
    the_engine_renders_active_zones_only(&node, &zone_id).await;
    unreadable_messages_close_the_connection(&mut socket).await;

    // Once nobody watches, the engine isn't asked for them any more
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let inputs = node.get("/api/v1/sources/local/inputs").await.json();
        if inputs["default_output"] == "signals: levels:" {
            break;
        }
        assert!(Instant::now() < deadline, "still asked: {inputs}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(node.stop().await, 0);
}

/// Waits until the engine was asked for `levels:` as `inputs.list` reports it.
async fn asked_levels(node: &Running, levels: &str) {
    let wanted = format!("signals:beat,spectrum levels:{levels}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let inputs = node.get("/api/v1/sources/local/inputs").await.json();
        if inputs["default_output"] == wanted {
            return;
        }
        assert!(Instant::now() < deadline, "not asked {wanted}: {inputs}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Deactivating the zone takes its surface from `config.apply`: the levels
/// the engine is asked for follow the assigned surfaces.
async fn the_engine_renders_active_zones_only(node: &Running, zone_id: &str) {
    let off = node
        .send_json(
            "POST",
            &format!("/api/v1/zones/{zone_id}/deactivate"),
            &json!({}),
        )
        .await;
    assert_eq!(off.json()["active"], false);
    asked_levels(node, "").await;
    let on = node
        .send_json(
            "POST",
            &format!("/api/v1/zones/{zone_id}/activate"),
            &json!({}),
        )
        .await;
    assert_eq!(on.json()["active"], true);
    asked_levels(node, "0").await;
}

/// Requests to the source go through its link.
async fn requests_reach_the_source(node: &Running) {
    let inputs = node.get("/api/v1/sources/local/inputs").await;
    assert_eq!(inputs.status, 200);
    conforms::<InputList>(&inputs.json(), "/inputs");
    assert_eq!(
        inputs.json()["default_output"],
        "signals: levels:",
        "nothing asked yet"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let media = loop {
        let media = node.get("/api/v1/sources/local/media").await;
        if media.status == 200 || Instant::now() > deadline {
            break media;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    conforms::<MediaState>(&media.json(), "/media");
    let art = node.get("/api/v1/sources/local/media/art").await;
    assert_eq!(art.status, 200);
    assert_eq!(art.header("content-type"), Some("image/png"));
    assert_eq!(art.body, ART);
    let control = |action: &str| json!({"action": action});
    let path = "/api/v1/sources/local/media/control";
    assert_eq!(
        node.send_json("POST", path, &control("next")).await.status,
        204
    );
    let refused = node.send_json("POST", path, &control("previous")).await;
    assert_eq!(
        (refused.status, refused.code()),
        (409, json!("not_available"))
    );
    assert_eq!(
        node.send_json("POST", path, &control("louder"))
            .await
            .status,
        400
    );
    assert_eq!(
        node.get("/api/v1/sources/elsewhere/inputs").await.status,
        404
    );
}

/// Every kind of data arrives: status with the source's parts, levels,
/// the spectrum and the surface, each with its topic's index.
async fn every_kind_of_data_arrives(socket: &mut Live, zone_id: &str) {
    let topics = json!({"subscribe": [
        "status", "beat:local", "input:local", "media:local", "levels:local",
        "spectrum:local", "surface:main", format!("zone:{zone_id}"), "target:lamp",
    ]});
    socket
        .send(Message::Text(topics.to_string().into()))
        .await
        .unwrap();
    let Message::Text(subscribed) = next(socket).await else {
        panic!("subscribed comes first");
    };
    let _ = std::fs::remove_dir_all(captured());
    std::fs::create_dir_all(captured()).unwrap();
    capture("subscribed--node.json", subscribed.as_bytes());
    let subscribed: Value = serde_json::from_str(&subscribed).unwrap();
    assert_eq!(subscribed["type"], "subscribed");
    let indices = &subscribed["data"]["topics"];
    assert_eq!(indices["spectrum:local"], json!({"index": 0}));
    assert_eq!(indices["surface:main"], json!({"index": 1}));
    assert_eq!(indices["target:lamp"], json!({"index": 2}));
    assert_eq!(indices["status"], json!({}));
    let (mut status, mut levels, mut spectrum, mut surface) = (None, None, false, false);
    let deadline = Instant::now() + Duration::from_secs(20);
    while status.is_none() || levels.is_none() || !spectrum || !surface {
        assert!(
            Instant::now() < deadline,
            "status {status:?}, levels {levels:?}, spectrum {spectrum}, surface {surface}"
        );
        match next(socket).await {
            Message::Text(text) => {
                let message: Value = serde_json::from_str(&text).unwrap();
                let local = &message["data"]["sources"]["local"];
                if message["type"] == "status"
                    && local.get("beat").is_some()
                    && local.get("input").is_some()
                    && local.get("media").is_some()
                {
                    capture("status--node.json", text.as_bytes());
                    status = Some(message);
                } else if message["type"] == "levels" {
                    capture("levels--node.json", text.as_bytes());
                    levels = Some(message);
                }
            }
            Message::Binary(bytes) => match &bytes[..4] {
                b"FOLG" => {
                    assert_eq!(&bytes[6..8], &[0, 0], "the spectrum's index");
                    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), BANDS);
                    capture("signalFrame--node.folg", &bytes);
                    spectrum = true;
                }
                b"FOLS" => {
                    assert_eq!(&bytes[8..10], &[1, 0], "the surface's index");
                    assert_eq!(bytes.len(), 28 + 10 * 3);
                    capture("surfaceFrame--node.fols", &bytes);
                    surface = true;
                }
                other => panic!("an unknown frame {other:?}"),
            },
            _ => {}
        }
    }
    let status = status.unwrap();
    conforms::<LiveStatusMessage>(&status, "a status message");
    let data = &status["data"];
    assert_eq!(data["sources"]["local"]["state"], "connected");
    assert_eq!(data["sources"]["local"]["beat"]["locked"], true);
    assert_eq!(
        data["zones"][zone_id],
        json!({"active": true, "ambient": 0, "quiet_s": 0}),
        "a new zone is active"
    );
    let levels = levels.unwrap();
    conforms::<LiveLevelsMessage>(&levels, "a levels message");
    assert_eq!(levels["data"]["surface"], "main");
    assert_eq!(levels["data"]["source"], "local");
}

/// A document written elsewhere is announced.
async fn documents_written_are_announced(node: &Running, socket: &mut Live) {
    let show = node.get("/api/v1/show").await.json();
    let written = node.send_json("PUT", "/api/v1/show", &show).await.json();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no config_changed");
        if let Message::Text(text) = next(socket).await {
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["type"] == "config_changed" && message["data"]["document"] == "show" {
                assert_eq!(message["data"]["revision"], written["revision"]);
                capture("configChanged--node.json", text.as_bytes());
                break;
            }
        }
    }
}

/// A message that isn't one of the three closes the connection.
async fn unreadable_messages_close_the_connection(socket: &mut Live) {
    socket
        .send(Message::Text("{\"hello\": true}".into()))
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let close = loop {
        assert!(Instant::now() < deadline, "not closed");
        if let Message::Close(frame) = next(socket).await {
            break frame;
        }
    };
    assert_eq!(close.unwrap().code, CloseCode::Policy);
}

// ------------------------------------------------------------------ zone activation

fn zone_on(name: &str, instance: &str, start: u64, count: u64) -> Value {
    json!({"name": name, "targets": [{
        "id": format!("target-{name}"), "kind": "line",
        "provider": {"instance": instance, "address": {"start": start, "count": count}},
    }]})
}

async fn add_zone(node: &Running, zone: &Value) -> String {
    let created = node.send_json("POST", "/api/v1/zones", zone).await;
    assert_eq!(created.status, 201);
    created.json()["id"].as_str().unwrap().to_owned()
}

async fn zones_revision(node: &Running) -> u64 {
    node.get("/api/v1/zones").await.json()["revision"]
        .as_u64()
        .unwrap()
}

async fn stored_active(node: &Running, id: &str) -> bool {
    node.get(&format!("/api/v1/zones/{id}")).await.json()["active"]
        .as_bool()
        .unwrap()
}

/// `POST /zones/{id}/activate` or `/deactivate`, checked against `ZoneActivation`.
async fn toggle(node: &Running, id: &str, verb: &str, body: &Value) -> Value {
    let answer = node
        .send_json("POST", &format!("/api/v1/zones/{id}/{verb}"), body)
        .await;
    assert_eq!(
        answer.status,
        200,
        "{verb}: {}",
        String::from_utf8_lossy(&answer.body)
    );
    let json = answer.json();
    conforms::<ZoneActivation>(&json, "a zone activation");
    assert_eq!(json["zone"], id);
    json
}

#[tokio::test(flavor = "multi_thread")]
async fn zones_are_activated_with_conflicts_and_take_over() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, false).await;
    // b shares LEDs with a: written after it, it comes up inactive and is refused nothing
    let a = add_zone(&node, &zone_on("a", "wled", 0, 10)).await;
    let b = add_zone(&node, &zone_on("b", "wled", 5, 10)).await;
    let c = add_zone(&node, &zone_on("c", "other", 0, 10)).await;
    assert!(stored_active(&node, &b).await, "GET gives the stored flag");

    let revision = zones_revision(&node).await;
    let nothing = json!({});
    assert_eq!(
        toggle(&node, &b, "activate", &nothing).await,
        json!({"zone": b, "active": false, "conflicts": [a]}),
        "a stored-active zone the check suppressed gets its holder as conflict"
    );
    assert_eq!(
        toggle(&node, &a, "activate", &nothing).await,
        json!({"zone": a, "active": true, "conflicts": []})
    );
    assert_eq!(
        toggle(&node, &c, "activate", &nothing).await["active"],
        true
    );
    assert_eq!(zones_revision(&node).await, revision, "nothing was written");

    let taken = toggle(&node, &b, "activate", &json!({"take_over": true})).await;
    assert_eq!(taken, json!({"zone": b, "active": true, "conflicts": []}));
    assert!(
        !stored_active(&node, &a).await,
        "the loser is deactivated entirely"
    );
    assert!(
        stored_active(&node, &c).await,
        "zones without shared targets stay"
    );
    assert_eq!(zones_revision(&node).await, revision + 1);

    // a is inactive and conflicts with b again
    assert_eq!(
        toggle(&node, &a, "activate", &nothing).await,
        json!({"zone": a, "active": false, "conflicts": [b]})
    );
    let off = toggle(&node, &b, "deactivate", &nothing).await;
    assert_eq!(off, json!({"zone": b, "active": false, "conflicts": []}));
    assert_eq!(zones_revision(&node).await, revision + 2);
    toggle(&node, &b, "deactivate", &nothing).await;
    assert_eq!(zones_revision(&node).await, revision + 2, "already off");
    assert_eq!(
        toggle(&node, &a, "activate", &nothing).await["active"],
        true
    );
    // An empty body is no take_over
    let empty = node
        .call(
            "POST",
            &format!("/api/v1/zones/{b}/activate"),
            &[("Content-Type", "application/json")],
            None,
        )
        .await;
    assert_eq!(empty.json()["conflicts"], json!([a]));

    // A zone the check suppressed takes its target over too, and the holder goes
    let extra = add_zone(&node, &zone_on("e", "wled", 6, 2)).await;
    let d = add_zone(&node, &zone_on("d", "wled", 0, 8)).await;
    assert_eq!(
        toggle(&node, &d, "activate", &nothing).await["conflicts"],
        json!([a])
    );
    let taken = toggle(&node, &d, "activate", &json!({"take_over": true})).await;
    assert_eq!(taken["active"], true);
    assert!(!stored_active(&node, &a).await);
    assert!(
        !stored_active(&node, &extra).await,
        "extra was suppressed, and goes too"
    );
    toggle(&node, &a, "activate", &json!({"take_over": true})).await;

    for verb in ["activate", "deactivate"] {
        let missing = node
            .send_json("POST", &format!("/api/v1/zones/nope/{verb}"), &nothing)
            .await;
        assert_eq!(missing.status, 404, "{verb}");
        assert_eq!(missing.code(), "not_found");
    }
    for bad in [
        json!({"take_over": "yes"}),
        json!({"other": true}),
        json!([]),
    ] {
        let refused = node
            .send_json("POST", &format!("/api/v1/zones/{a}/activate"), &bad)
            .await;
        assert_eq!(refused.status, 400, "{bad}");
    }
    assert_eq!(node.stop().await, 0);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(home.0.join("zones.json")).unwrap()).unwrap();
    assert_eq!(
        saved["zones"][0]["active"], true,
        "stored when the node stopped"
    );
}

/// Reads live messages until a `status` satisfies `wanted`.
async fn status_until(socket: &mut Live, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no such status");
        if let Message::Text(text) = next(socket).await {
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["type"] == "status" && wanted(&message["data"]) {
                conforms::<LiveStatusMessage>(&message, "a status message");
                return message;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_live_channel_reports_the_effective_activation() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let home = Home::new();
    let node = Running::start(&home, false).await;
    let a = add_zone(&node, &zone_on("a", "wled", 0, 10)).await;
    let b = add_zone(&node, &zone_on("b", "wled", 5, 10)).await;
    let url = format!("ws://127.0.0.1:{}/api/v1/live", node.port);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let topics = json!({"subscribe": ["status", format!("zone:{b}")]});
    socket
        .send(Message::Text(topics.to_string().into()))
        .await
        .unwrap();

    // b's stored flag is true, but a holds the target
    let (ka, kb) = (a.clone(), b.clone());
    status_until(&mut socket, |d| {
        d["zones"][&ka]["active"] == true && d["zones"][&kb]["active"] == false
    })
    .await;
    assert!(stored_active(&node, &b).await);

    toggle(&node, &a, "deactivate", &json!({})).await;
    let (ka, kb) = (a.clone(), b.clone());
    status_until(&mut socket, |d| {
        d["zones"][&ka]["active"] == false && d["zones"][&kb]["active"] == true
    })
    .await;
    assert_eq!(node.stop().await, 0);
}
