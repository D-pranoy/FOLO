//! RB-10 step 5: a migrated v1 configuration drives a fake Hyperion through the node's real
//! output path, and the `color` lines it records are the bytes the legacy sender writes for the
//! same frames (`tests/golden/hyperion/parity.json`, made by the legacy `JsonClient` from the
//! renderer goldens, with the legacy origin "Audio Effects").
//!
//! The fake engine is this test binary run again by the node, as in `api.rs`: the test
//! `fake_engine_replay` acts as an engine when the link's variables are set. It sends the
//! plan's frames one per interval once the test has made the `go` file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folo_node::cli::{LocationArgs, RunArgs};
use folo_provider_hyperion::testing::FakeHyperion;
use folo_supervisor::EngineCommand;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

/// Where the test tells the engine what to send (the engine is a process of its own).
const SCRATCH: &str = env!("CARGO_TARGET_TMPDIR");
const PLAN: &str = "hyperion-parity-plan.json";
const GO: &str = "hyperion-parity-go";
/// Longer than a sender tick (25 ms at 40 Hz) and a node step, so every frame is sent.
const INTERVAL_MS: u64 = 150;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rpc(value: &Value) -> Message {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(value.to_string().as_bytes());
    Message::Binary(bytes.into())
}

/// A surface frame (type byte included) of rgb8 cells.
fn surface_frame(index: u16, sequence: u32, levels: &[u8]) -> Message {
    let width = u16::try_from(levels.len() / 3).unwrap();
    let mut bytes = vec![0x02];
    bytes.extend_from_slice(b"FOLS");
    bytes.extend_from_slice(&[1, 1, 2, 0]);
    bytes.extend_from_slice(&index.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(levels);
    Message::Binary(bytes.into())
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
    let plan: Value = serde_json::from_slice(
        &std::fs::read(Path::new(SCRATCH).join(PLAN)).expect("the test wrote the plan"),
    )
    .unwrap();
    let rows: Vec<Vec<u8>> = plan["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
                .collect()
        })
        .collect();
    let mut surface: Option<u16> = None;
    let mut next_row = 0usize;
    let mut told_active = false;
    let mut tick = tokio::time::interval(Duration::from_millis(INTERVAL_MS));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let go = Path::new(SCRATCH).join(GO).exists();
                let Some(index) = surface.filter(|_| go) else { continue };
                if !told_active {
                    told_active = true;
                    let active = json!({"jsonrpc": "2.0", "method": "activity",
                        "params": {"active": true}});
                    if socket.send(rpc(&active)).await.is_err() {
                        return 1;
                    }
                }
                if let Some(row) = rows.get(next_row) {
                    let sequence = u32::try_from(next_row + 1).unwrap();
                    if socket.send(surface_frame(index, sequence, row)).await.is_err() {
                        return 1;
                    }
                    next_row += 1;
                }
            }
            message = socket.next() => {
                let Some(Ok(Message::Binary(bytes))) = message else { return 1 };
                let request: Value = serde_json::from_slice(&bytes[1..]).unwrap();
                if request["method"] == "config.apply" {
                    surface = request["params"]["publish"]["surfaces"][0]["index"]
                        .as_u64()
                        .and_then(|i| u16::try_from(i).ok());
                }
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
fn fake_engine_replay() {
    let (Ok(url), Ok(token)) = (
        std::env::var("FOLO_SOURCE_LINK"),
        std::env::var("FOLO_SOURCE_TOKEN"),
    ) else {
        return; // an ordinary test run
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    std::process::exit(runtime.block_on(engine(&url, &token)));
}

/// A home holding a migrated configuration's documents, pointed at the fake Hyperion.
struct Home(PathBuf);

impl Home {
    fn migrated(config: &str, hyperion: &FakeHyperion, origin: &str) -> Self {
        let path = std::env::temp_dir().join(format!("folo-parity-{}", folo_store::new_id()));
        let out = root()
            .join("tests/golden/migration")
            .join(config)
            .join("out");
        copy_dir(&out, &path);
        let providers_file = path.join("providers.json");
        let mut providers: Value = read_json(&providers_file);
        let instance = &mut providers["providers"][0];
        instance["enabled"] = json!(true);
        instance["settings"]["host"] = json!(hyperion.host());
        instance["settings"]["port"] = json!(hyperion.port());
        instance["settings"]["origin"] = json!(origin);
        std::fs::write(&providers_file, providers.to_string()).unwrap();
        // The migration names the target `strip`, and Hyperion's targets are its instance
        // numbers (hyperion.md): the legacy studio drove instance 0
        let zones_file = path.join("zones.json");
        let zones = std::fs::read_to_string(&zones_file)
            .unwrap()
            .replace("\"strip\"", "\"0\"");
        std::fs::write(&zones_file, zones).unwrap();
        Self(path)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn levels_of(line: &str) -> Vec<u8> {
    let command: Value = serde_json::from_str(line).unwrap();
    command["color"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
        .collect()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Runs the node on `home` with the built-in providers until the case's frames were sent, and
/// returns the `color` lines the fake Hyperion received, without their newline.
async fn run_case(config: &str, lines: &[String], origin: &str) -> Vec<String> {
    let hyperion = FakeHyperion::start().await;
    let home = Home::migrated(config, &hyperion, origin);
    let rows: Vec<Vec<u8>> = lines.iter().map(|l| levels_of(l)).collect();
    std::fs::write(
        Path::new(SCRATCH).join(PLAN),
        json!({"rows": rows}).to_string(),
    )
    .unwrap();
    let _ = std::fs::remove_file(Path::new(SCRATCH).join(GO));
    let args = RunArgs {
        port: Some(free_port().to_string()),
        ..RunArgs::default()
    };
    let location = LocationArgs {
        home: Some(home.0.clone()),
        service: false,
    };
    let command = EngineCommand {
        program: std::env::current_exe().unwrap(),
        args: [
            "fake_engine_replay",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let node = tokio::spawn(async move {
        folo_node::node::run_with(&location, &args, Some(command), async {
            let _ = stopped.await;
        })
        .await
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    while hyperion.connections() == 0 {
        assert!(
            Instant::now() < deadline,
            "{config}: the node never connected"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The sender is running once the node's idle black arrives
    while hyperion.color_frames().is_empty() {
        assert!(Instant::now() < deadline, "{config}: the sender never sent");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let before_go = hyperion.color_frames().len();
    std::fs::write(Path::new(SCRATCH).join(GO), b"").unwrap();
    // Until the last frame arrives, then a moment more: a source that goes quiet is stale after
    // a while, and the compositor then sends black, which is not what is compared here
    let last = lines.last().unwrap().trim_end().to_owned();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !hyperion.color_frames()[before_go..]
        .iter()
        .any(|r| r.line == last)
    {
        assert!(
            Instant::now() < deadline,
            "{config}: the last frame never arrived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = stop.send(());
    assert_eq!(node.await.unwrap(), 0);
    let _ = std::fs::remove_file(Path::new(SCRATCH).join(GO));
    hyperion
        .color_frames()
        .into_iter()
        .map(|received| received.line)
        .collect()
}

/// Consecutive repeats are keepalives, which the legacy sender also writes.
fn without_repeats(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if out.last() != Some(&line) {
            out.push(line);
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_migrated_configuration_sends_the_legacy_bytes_to_hyperion() {
    let golden = read_json(&root().join("tests/golden/hyperion/parity.json"));
    let origin = golden["origin"].as_str().unwrap();
    for case in golden["cases"].as_array().unwrap() {
        let config = case["config"].as_str().unwrap();
        let png = case["png"].as_str().unwrap();
        let expected: Vec<String> = case["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap().trim_end().to_owned())
            .collect();
        let mut sent = without_repeats(run_case(config, &expected, origin).await);
        // Before the engine's first frame the node sends the idle black, as the studio's
        // sender does for silence: that is not one of the compared frames
        let dark = |line: &String| levels_of(line).iter().all(|v| *v == 0);
        if sent.first().is_some_and(dark) && !expected.first().is_some_and(dark) {
            sent.remove(0);
        }
        assert_eq!(
            sent.len(),
            expected.len(),
            "{config} {png}: how many frames"
        );
        for (i, (got, want)) in sent.iter().zip(&expected).enumerate() {
            assert_eq!(got, want, "{config} {png}: frame {i}");
        }
    }
}
