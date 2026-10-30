//! The test doubles of the Hue tests: a bridge on loopback and its DTLS endpoint.
#![allow(dead_code)]

pub mod certs;
pub mod dtls_server;
pub mod fake_bridge;

use std::sync::{Mutex, Once};

use folo_provider_hue::bridge::Probe;
use folo_provider_hue::trust::Anchors;
use folo_provider_hue::{HueKind, HueOptions};
use serde_json::json;

use fake_bridge::{BRIDGE_ID, CLIENT_KEY, FakeBridge, USERNAME};

/// Everything the `log` crate was asked to write, so a test can look for secrets in it.
static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        LOGGED
            .lock()
            .unwrap()
            .push(format!("{}: {}", record.target(), record.args()));
    }

    fn flush(&self) {}
}

pub fn capture_logs() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = log::set_logger(&Capture);
        log::set_max_level(log::LevelFilter::Trace);
    });
}

pub fn logged() -> Vec<String> {
    LOGGED.lock().unwrap().clone()
}

/// Options that reach the fake bridge: its authority is the only root, retransmits are quick,
/// and nothing goes to the internet.
pub fn options(bridge: &FakeBridge) -> HueOptions {
    let mut options = HueOptions {
        anchors: Anchors::Custom(vec![bridge.authority_der.clone()]),
        dtls_port: bridge.endpoint.dtls_port,
        dtls_retransmit_min: std::time::Duration::from_millis(150),
        dtls_retransmit_max: std::time::Duration::from_millis(250),
        action_pause: std::time::Duration::from_millis(20),
        ..HueOptions::default()
    };
    options.rest.backoff = std::time::Duration::from_millis(10);
    options.discovery.cloud_url = None;
    options.discovery.mdns_timeout = std::time::Duration::from_millis(50);
    options
}

pub fn instance(bridge: &FakeBridge, id: &str) -> folo_protocol::generated::ProviderInstance {
    serde_json::from_value(json!({
        "id": id, "kind": "hue", "name": "Bridge", "rate_hz": 50,
        "settings": {
            "kind": "hue", "bridge_id": BRIDGE_ID,
            "address": format!("127.0.0.1:{}", bridge.endpoint.https_port),
            "streaming_mode": "linked",
        },
    }))
    .unwrap()
}

pub fn secrets() -> folo_protocol::generated::ProviderSecrets {
    serde_json::from_value(json!({"app_key": USERNAME, "client_key": CLIENT_KEY})).unwrap()
}

/// A Hue provider for the fake bridge, with the secrets from pairing.
pub fn provider(bridge: &FakeBridge) -> std::sync::Arc<dyn folo_provider::Provider> {
    use folo_provider::ProviderKind;
    HueKind::new(options(bridge))
        .create(&instance(bridge, "hue1"), Some(&secrets()))
        .unwrap()
}

pub fn unused(_: &Probe) {}
