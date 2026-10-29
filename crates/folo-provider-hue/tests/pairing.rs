//! Pairing through the link button.

mod support;

use std::time::Duration;

use folo_protocol::generated::HuePairingState;
use folo_provider_hue::bridge::client_for;
use folo_provider_hue::endpoint::Endpoint;
use folo_provider_hue::pairing::{PairingError, PairingProgress, PairingSettings, pair};
use folo_provider_hue::rest::{BridgeClient, RestLimits};
use folo_provider_hue::trust::{Anchors, BridgeTrust};
use support::fake_bridge::{APPLICATION_ID, BRIDGE_ID, CLIENT_KEY, FakeBridge, USERNAME};

fn client(bridge: &FakeBridge) -> BridgeClient {
    let trust = BridgeTrust {
        bridge_id: BRIDGE_ID.into(),
        anchors: Anchors::Custom(vec![bridge.authority_der.clone()]),
        pinned: None,
    };
    client_for(&bridge.endpoint, trust, None, RestLimits::default()).unwrap()
}

fn quick(window_ms: u64) -> PairingSettings {
    let mut settings = PairingSettings::new("test-host");
    settings.window = Duration::from_millis(window_ms);
    settings.poll = Duration::from_millis(20);
    settings
}

#[tokio::test]
async fn pairing_waits_for_the_button_then_yields_the_credentials() {
    let bridge = FakeBridge::start().await;
    bridge.state().presses_after = 3;
    let mut progress = Vec::new();
    let credentials = pair(&client(&bridge), &quick(5000), &mut |p| progress.push(p))
        .await
        .unwrap();

    assert_eq!(credentials.app_key, USERNAME);
    assert_eq!(credentials.client_key, CLIENT_KEY.to_uppercase());
    assert_eq!(credentials.application_id, APPLICATION_ID);
    assert_eq!(bridge.state().pair_attempts, 4);
    let waiting = progress
        .iter()
        .filter(|p| matches!(p, PairingProgress::Waiting { .. }))
        .count();
    assert_eq!(waiting, 3);
    assert_eq!(progress.last(), Some(&PairingProgress::Paired));
    assert_eq!(
        progress.last().unwrap().api().state,
        HuePairingState::Paired
    );
    assert_eq!(progress[0].api().state, HuePairingState::Waiting);

    // The device type is `folo#<host>`, and key generation was asked for
    let (_, _, body) = bridge
        .state()
        .requests
        .iter()
        .find(|(m, p, _)| m == "POST" && p == "/api")
        .cloned()
        .unwrap();
    assert_eq!(body["devicetype"], "folo#test-host");
    assert_eq!(body["generateclientkey"], true);
}

#[tokio::test]
async fn pairing_gives_up_when_the_button_is_not_pressed() {
    let bridge = FakeBridge::start().await;
    bridge.state().presses_after = u32::MAX;
    let error = pair(&client(&bridge), &quick(150), &mut |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairingError::TimedOut);
}

#[tokio::test]
async fn the_credentials_become_secrets_and_never_show_in_debug_output() {
    let bridge = FakeBridge::start().await;
    let credentials = pair(&client(&bridge), &quick(5000), &mut |_| {})
        .await
        .unwrap();
    let secrets = credentials.secrets().unwrap();
    assert_eq!(
        secrets.app_key.as_deref().map(String::as_str),
        Some(USERNAME)
    );
    assert!(secrets.client_key.is_some());
    let shown = format!("{credentials:?} {:?}", client(&bridge));
    assert!(
        !shown.contains(USERNAME) && !shown.contains(&CLIENT_KEY.to_uppercase()),
        "{shown}"
    );
}

#[tokio::test]
async fn an_unreachable_bridge_is_reported_as_such() {
    let bridge = FakeBridge::start().await;
    let mut endpoint = bridge.endpoint.clone();
    endpoint.https_port = 1;
    let trust = BridgeTrust {
        bridge_id: BRIDGE_ID.into(),
        anchors: Anchors::Custom(vec![bridge.authority_der.clone()]),
        pinned: None,
    };
    let client = client_for(&Endpoint { ..endpoint }, trust, None, RestLimits::default()).unwrap();
    let error = pair(&client, &quick(500), &mut |_| {}).await.unwrap_err();
    assert!(matches!(error, PairingError::Unreachable(_)), "{error}");
}
