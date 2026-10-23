//! The node with the real effect engine (RB-06 step 6 and 9, RB-07 step 4):
//! a first run imports a legacy `studio.json` through `folo-source
//! migrate-v1`, the engine links and takes its configuration, the live
//! channel streams what it renders, and it stops when asked.
//!
//! It runs when `FOLO_E2E=1` and `folo-source` is on the `PATH` (CI's Rust
//! jobs set both up with uv); otherwise it says so and passes.

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folo_node::cli::{LocationArgs, RunArgs};
use folo_node::client;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/golden/migration/default"
);

fn without_stamps(mut document: Value) -> Value {
    if let Value::Object(map) = &mut document {
        map.remove("revision");
        map.remove("server_id");
    }
    document
}

/// The live channel streams what the engine renders: the spectrum, the
/// migrated surface `main`, its layers' levels, and the input and beat.
async fn live_channel_streams_the_engine(port: u16) {
    let url = format!("ws://127.0.0.1:{port}/api/v1/live");
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let topics = json!({"subscribe": ["status", "input:local", "beat:local", "levels:local", "spectrum:local", "surface:main"]});
    socket
        .send(Message::Text(topics.to_string().into()))
        .await
        .unwrap();
    let (mut spectrum, mut surface, mut levels, mut status) = (false, false, false, false);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !(spectrum && surface && levels && status) {
        assert!(
            Instant::now() < deadline,
            "spectrum {spectrum}, surface {surface}, levels {levels}, status {status}"
        );
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("live data in time")
            .unwrap()
            .unwrap();
        match message {
            Message::Binary(bytes) if bytes.starts_with(b"FOLG") => {
                assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 192);
                spectrum = true;
            }
            Message::Binary(bytes) if bytes.starts_with(b"FOLS") => {
                // The default config's 150 LEDs as rgb8
                assert_eq!(bytes.len(), 28 + 150 * 3);
                surface = true;
            }
            Message::Text(text) => {
                let message: Value = serde_json::from_str(&text).unwrap();
                let local = &message["data"]["sources"]["local"];
                match message["type"].as_str() {
                    Some("levels") => {
                        assert_eq!(message["data"]["surface"], "main");
                        levels = !message["data"]["layers"].as_object().unwrap().is_empty();
                    }
                    Some("status") => {
                        status = local["state"] == "connected"
                            && local["input"].is_object()
                            && local["beat"].is_object();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

async fn status(port: u16) -> Option<Value> {
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

#[tokio::test(flavor = "multi_thread")]
async fn the_real_engine_links_after_a_first_run_import() {
    if std::env::var("FOLO_E2E").as_deref() != Ok("1") {
        eprintln!("skipped: set FOLO_E2E=1 with folo-source on the PATH");
        return;
    }
    let home = std::env::temp_dir().join(format!("folo-e2e-{}", folo_store::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let golden = PathBuf::from(GOLDEN);
    std::fs::copy(golden.join("studio.json"), home.join("studio.json")).unwrap();
    let port = StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let location = LocationArgs {
        home: Some(home.clone()),
        service: false,
    };
    let args = RunArgs {
        port: Some(port.to_string()),
        ..RunArgs::default()
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let node = tokio::spawn(async move {
        folo_node::node::run(&location, &args, async {
            let _ = stopped.await;
        })
        .await
    });

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = status(port).await
            && status["workers"]["folo-source"]["state"] == "running"
        {
            break;
        }
        assert!(!node.is_finished(), "the node ended early");
        assert!(Instant::now() < deadline, "the engine didn't link");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for name in ["zones.json", "show.json", "providers.json", "sources.json"] {
        assert_eq!(
            without_stamps(read_json(&home.join(name))),
            without_stamps(read_json(&golden.join("out").join(name))),
            "{name}"
        );
    }
    assert!(home.join("studio.json.v1.bak").is_file());

    let log = home.join("logs").join("folo.log");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !std::fs::read_to_string(&log)
        .unwrap()
        .contains("applied configuration 1")
    {
        assert!(
            Instant::now() < deadline,
            "the engine didn't apply its configuration"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    live_channel_streams_the_engine(port).await;

    let _ = stop.send(());
    let status = tokio::time::timeout(Duration::from_secs(30), node)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, 0);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("the effect engine stopped"), "{text}");
    let _ = std::fs::remove_dir_all(&home);
}
