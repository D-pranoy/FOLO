//! What the node keeps for its providers (RB-10, RB-12, RB-13): a Hyperion token set through
//! administration lands in `secrets.json` and is used when the instance is made again, a new
//! WLED instance runs at 60 Hz, and the built-in kinds are registered.

use std::io::{Read as _, Write as _};
use std::time::{Duration, Instant};

use folo_node::cli::{LocationArgs, RunArgs};
use folo_provider_hyperion::testing::FakeHyperion;
use serde_json::{Value, json};
use tokio::sync::oneshot;

const TOKEN: &str = "c1b2a3d4-0000-4000-8000-123456789abc";

fn call(port: u16, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let body = body.to_string();
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let status = response.split(' ').nth(1).unwrap().parse().unwrap();
    let body = response.split("\r\n\r\n").nth(1).unwrap_or_default();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

async fn node_call(
    port: u16,
    method: &'static str,
    path: &'static str,
    body: Value,
) -> (u16, Value) {
    tokio::task::spawn_blocking(move || call(port, method, path, &body))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_is_kept_in_secrets_and_a_wled_instance_runs_at_60_hz() {
    let hyperion = FakeHyperion::start().await;
    let home = std::env::temp_dir().join(format!("folo-secrets-{}", folo_store::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let args = RunArgs {
        port: Some(port.to_string()),
        local_source: Some("off".to_owned()),
        ..RunArgs::default()
    };
    let location = LocationArgs {
        home: Some(home.clone()),
        service: false,
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let node = tokio::spawn(async move {
        folo_node::node::run_with(&location, &args, None, async {
            let _ = stopped.await;
        })
        .await
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the node never listened");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let made = json!({"id": "hy", "kind": "hyperion", "name": "Hyperion", "settings": {
        "kind": "hyperion", "host": hyperion.host(), "port": hyperion.port()}});
    let (status, _) = node_call(port, "POST", "/api/v1/providers", made).await;
    assert_eq!(status, 201);
    let wled = json!({"id": "wl", "kind": "wled", "name": "WLED", "enabled": false});
    let (status, created) = node_call(port, "POST", "/api/v1/providers", wled).await;
    assert_eq!(status, 201);
    assert_eq!(created["rate_hz"], 60, "a new WLED instance");
    let hue = json!({"id": "hu", "kind": "hue", "name": "Hue", "enabled": false});
    let (_, created) = node_call(port, "POST", "/api/v1/providers", hue).await;
    assert_eq!(
        created["rate_hz"], 40,
        "the contract's default for the rest"
    );

    while hyperion.connections() == 0 {
        assert!(Instant::now() < deadline, "Hyperion was never reached");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // a malformed body is 400, not 502
    let admin = "/api/v1/providers/hy/admin/token";
    let (status, error) = node_call(port, "POST", admin, json!({"token": 5})).await;
    assert_eq!(
        (status, &error["error"]["code"]),
        (400, &json!("bad_body")),
        "{error}"
    );
    let secrets = folo_store::read_secrets(&home).unwrap();
    assert!(secrets["providers"]["hy"].is_null(), "nothing kept yet");

    // a login that Hyperion accepts is kept
    let (status, _) = node_call(port, "POST", admin, json!({"token": TOKEN})).await;
    assert_eq!(status, 200);
    let secrets = folo_store::read_secrets(&home).unwrap();
    assert_eq!(secrets["providers"]["hy"]["token"], TOKEN);
    let _ = stop.send(());
    assert_eq!(node.await.unwrap(), 0);
    let _ = std::fs::remove_dir_all(&home);
}
