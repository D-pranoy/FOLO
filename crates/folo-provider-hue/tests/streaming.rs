//! Connecting, describing, and streaming to an entertainment area, through the provider traits
//! and the real sender, against the fake bridge and its DTLS endpoint.

mod support;

use std::sync::Arc;
use std::time::Duration;

use folo_protocol::generated::{ProviderSecrets, TargetKind};
use folo_provider::{
    AdminMethod, Delivery, ElementColors, ElementFrame, Provider, ProviderError, ProviderKind,
    ProviderState, SenderConfig, SenderTarget, TargetOutput, spawn_sender,
};
use folo_provider_hue::HueKind;
use serde_json::{Value, json};
use support::fake_bridge::{AREA, BRIDGE_ID, CLIENT_KEY, FakeBridge, USERNAME};

const LAMP: &str = "11111111-2222-3333-4444-555555555555/svc-lamp";
const STRIP: &str = "11111111-2222-3333-4444-555555555555/svc-strip";
/// `HueStream`, then version 2.0.
const HEADER: &str = "48756553747265616d0200";
const AREA_ID_HEX: &str =
    "31313131313131312d323232322d333333332d343434342d353535353535353535353535";

fn output(provider: &Arc<dyn Provider>) -> &dyn folo_provider::ElementOutput {
    provider.element_output().unwrap()
}

fn lamp(r: f32, g: f32, b: f32) -> TargetOutput {
    TargetOutput {
        target: LAMP.into(),
        colors: ElementColors::Rgb(vec![[r, g, b]]),
    }
}

fn strip(colors: [[f32; 3]; 3]) -> TargetOutput {
    TargetOutput {
        target: STRIP.into(),
        colors: ElementColors::Rgb(colors.to_vec()),
    }
}

fn frame(targets: Vec<TargetOutput>) -> ElementFrame {
    ElementFrame::new(0, targets)
}

/// The packet a bridge should get, written out from the layout in docs/integrations/hue.md.
fn golden(sequence: u8, records: &str) -> Vec<u8> {
    hex::decode(format!(
        "{HEADER}{sequence:02x}00000000{AREA_ID_HEX}{records}"
    ))
    .unwrap()
}

async fn connected(bridge: &FakeBridge) -> Arc<dyn Provider> {
    let provider = support::provider(bridge);
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();
    provider
}

#[tokio::test]
async fn describe_maps_lights_to_point_and_line_targets_by_channel_id() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let described = provider.describe().await.unwrap();
    let by_id = |id: &str| described.iter().find(|t| t.id == id).unwrap();

    let lamp = by_id(LAMP);
    assert_eq!(lamp.kind, TargetKind::Point);
    assert_eq!(lamp.address.channels, vec![0]);
    assert_eq!(lamp.name, "Desk lamp");
    let strip = by_id(STRIP);
    assert_eq!(strip.kind, TargetKind::Line);
    assert_eq!(strip.address.channels, vec![1, 2, 3]);
    assert_eq!(strip.elements, 3);
    assert_eq!(strip.name, "Gradient strip");
}

#[tokio::test]
async fn the_delivery_policy_fits_the_bridge() {
    let bridge = FakeBridge::start().await;
    let policy = support::provider(&bridge).delivery_policy();
    assert_eq!(
        policy.keepalive,
        Duration::from_secs(5),
        "inside the ten seconds the bridge waits"
    );
}

#[tokio::test]
async fn an_instance_that_is_not_paired_or_has_the_wrong_key_is_refused() {
    let bridge = FakeBridge::start().await;
    let kind = HueKind::new(support::options(&bridge));
    let instance = support::instance(&bridge, "hue1");

    let unpaired = kind.create(&instance, None).unwrap();
    assert!(
        matches!(unpaired.connect().await, Err(ProviderError::Refused(m)) if m.contains("link button"))
    );

    let secrets: ProviderSecrets =
        serde_json::from_value(json!({"app_key": "stale-key", "client_key": CLIENT_KEY})).unwrap();
    let stale = kind.create(&instance, Some(&secrets)).unwrap();
    assert!(
        matches!(stale.connect().await, Err(ProviderError::Refused(m)) if m.contains("pair again"))
    );
}

#[tokio::test]
async fn a_bridge_that_cannot_stream_or_is_another_one_is_refused() {
    let bridge = FakeBridge::start().await;
    bridge.state().api_version = "1.21.0".into();
    let provider = support::provider(&bridge);
    assert!(
        matches!(provider.connect().await, Err(ProviderError::Refused(m)) if m.contains("1.22"))
    );

    bridge.state().api_version = "1.65.0".into();
    bridge.state().model = "BSB001".into();
    assert!(
        matches!(provider.connect().await, Err(ProviderError::Refused(m)) if m.contains("BSB001"))
    );

    bridge.state().model = "BSB002".into();
    let kind = HueKind::new(support::options(&bridge));
    let mut instance = support::instance(&bridge, "hue1");
    if let folo_protocol::generated::ProviderSettings::HueSettings(s) = &mut instance.settings {
        s.bridge_id = "001788fffe999999".parse().unwrap();
    }
    let other = kind.create(&instance, Some(&support::secrets())).unwrap();
    // Its certificate is for another bridge id, so it isn't even asked
    assert!(other.connect().await.is_err());
}

/// One channel's record: its id and three 16 bit levels made of duplicated bytes.
fn record(channel: u8, r: u8, g: u8, b: u8) -> String {
    format!("{channel:02x}{r:02x}{r:02x}{g:02x}{g:02x}{b:02x}{b:02x}")
}

#[tokio::test]
async fn streaming_sends_every_channel_in_every_packet_and_checks_the_decrypted_bytes() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let out = output(&provider);

    out.start(LAMP).await.unwrap();
    assert_eq!(bridge.area(AREA)["status"], "active");
    out.start(STRIP).await.unwrap();
    let starts = bridge
        .requests()
        .iter()
        .filter(|r| *r == &format!("PUT /clip/v2/resource/entertainment_configuration/{AREA}"))
        .count();
    assert_eq!(
        starts,
        1,
        "the second target joins the stream that runs: {:?}",
        bridge.requests()
    );

    // Truncated toward zero: 128.9 is 0x80, and 255.9 saturates at 0xff
    let first = frame(vec![
        lamp(255.9, 0.0, 128.9),
        strip([[10.0, 20.0, 30.0], [40.0, 50.0, 60.0], [70.0, 80.0, 90.0]]),
    ]);
    out.send(&first).await.unwrap();
    // Only the lamp changes: the strip's channels keep their colours in the packet
    out.send(&frame(vec![lamp(1.0, 2.0, 3.0)])).await.unwrap();
    bridge
        .dtls
        .eventually("two datagrams", |o| o.datagrams.len() == 2)
        .await;

    let datagrams = bridge.dtls.datagrams();
    // Written out by hand: channel 0 is the lamp, 1 to 3 the strip's segments in member order
    let literal = "00ffff00008080010a0a14141e1e02282832323c3c03464650505a5a";
    assert_eq!(datagrams[0], golden(0, literal));
    let second = [
        record(0, 1, 2, 3),
        record(1, 10, 20, 30),
        record(2, 40, 50, 60),
        record(3, 70, 80, 90),
    ]
    .concat();
    assert_eq!(
        datagrams[1],
        golden(1, &second),
        "every channel again, the sequence one up"
    );
    let identities = bridge.dtls.observed.lock().unwrap().identities.clone();
    assert_eq!(
        identities,
        vec![support::fake_bridge::APPLICATION_ID.to_owned()],
        "the DTLS identity is the application id"
    );
}

#[tokio::test]
async fn a_gradient_target_takes_its_elements_in_member_order_not_list_order() {
    let bridge = FakeBridge::start().await;
    // The bridge lists the strip's channels in another order than their member indexes
    {
        let mut state = bridge.state();
        let channels = state.configurations[0]["channels"].as_array_mut().unwrap();
        channels.swap(1, 3);
    }
    let provider = connected(&bridge).await;
    let out = output(&provider);
    out.start(STRIP).await.unwrap();
    out.send(&frame(vec![strip([
        [1.0, 1.0, 1.0],
        [2.0, 2.0, 2.0],
        [3.0, 3.0, 3.0],
    ])]))
    .await
    .unwrap();
    bridge
        .dtls
        .eventually("a datagram", |o| o.datagrams.len() == 1)
        .await;
    let packet = &bridge.dtls.datagrams()[0];
    let records = hex::encode(&packet[52..]);
    // Element 0 (member index 0) is channel 1, element 2 is channel 3: by channel id, whatever the list order
    let expected = [
        record(0, 0, 0, 0),
        record(1, 1, 1, 1),
        record(2, 2, 2, 2),
        record(3, 3, 3, 3),
    ]
    .concat();
    assert_eq!(records, expected);
}

#[tokio::test]
async fn the_stream_ends_with_the_last_release_close_notify_first_then_stop() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let out = output(&provider);
    out.start(LAMP).await.unwrap();
    out.start(STRIP).await.unwrap();

    out.release(LAMP).await.unwrap();
    assert_eq!(
        bridge.area(AREA)["status"],
        "active",
        "the strip still runs"
    );
    assert_eq!(bridge.dtls.observed.lock().unwrap().closes, 0);

    out.release(STRIP).await.unwrap();
    bridge
        .dtls
        .eventually("close_notify", |o| o.closes == 1)
        .await;
    assert_eq!(bridge.area(AREA)["status"], "inactive");
    let requests = bridge.requests();
    let stops = requests
        .iter()
        .filter(|r| r.starts_with("PUT /clip/v2/resource/entertainment_configuration/"))
        .count();
    assert_eq!(stops, 2, "one start and one stop: {requests:?}");
    assert_eq!(
        bridge.state().requests.last().unwrap().2,
        json!({"action": "stop"})
    );

    // Harmless for a target that isn't running
    out.release(LAMP).await.unwrap();
    out.release("no-such-target").await.unwrap();
}

#[tokio::test]
async fn a_target_of_another_area_waits_while_the_bridge_streams_to_one() {
    let bridge = FakeBridge::start().await;
    let mut other = bridge.area(AREA);
    other["id"] = json!("22222222-2222-3333-4444-555555555555");
    other["metadata"]["name"] = json!("Music");
    bridge.state().configurations.push(other);
    let provider = connected(&bridge).await;
    let out = output(&provider);
    out.start(LAMP).await.unwrap();
    let error = out
        .start("22222222-2222-3333-4444-555555555555/svc-lamp")
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Refused(m) if m.contains("one stream")));
}

#[tokio::test]
async fn start_is_retried_three_times_and_then_gives_up_without_a_handshake() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let out = output(&provider);

    bridge.state().start_failures = 2;
    out.start(LAMP).await.unwrap();
    let puts = bridge
        .requests()
        .iter()
        .filter(|r| r.starts_with("PUT"))
        .count();
    assert_eq!(puts, 3, "two failures and a success");
    out.release(LAMP).await.unwrap();

    let before = bridge.requests().len();
    bridge.state().start_failures = 5;
    assert!(out.start(LAMP).await.is_err());
    let attempts = bridge.requests()[before..]
        .iter()
        .filter(|r| r.starts_with("PUT"))
        .count();
    assert_eq!(attempts, 3);
    assert_eq!(
        bridge.dtls.observed.lock().unwrap().handshakes,
        1,
        "no handshake for a start that failed"
    );
}

#[tokio::test]
async fn a_handshake_that_fails_stops_the_area_again() {
    let bridge = FakeBridge::start().await;
    let kind = HueKind::new({
        let mut options = support::options(&bridge);
        options.dtls_attempts = 2;
        options
    });
    let wrong: ProviderSecrets = serde_json::from_value(
        json!({"app_key": USERNAME, "client_key": "ffffffffffffffffffffffffffffffff"}),
    )
    .unwrap();
    let provider = kind
        .create(&support::instance(&bridge, "hue1"), Some(&wrong))
        .unwrap();
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();
    let error = output(&provider).start(LAMP).await.unwrap_err();
    assert!(matches!(error, ProviderError::Failed(m) if m.contains("handshake")));
    assert_eq!(
        bridge.area(AREA)["status"],
        "inactive",
        "what start began is undone"
    );
}

#[tokio::test]
async fn too_many_channels_are_refused_before_anything_starts() {
    let bridge = FakeBridge::start().await;
    {
        let mut state = bridge.state();
        let channels = state.configurations[0]["channels"].as_array_mut().unwrap();
        for id in 4..21u8 {
            channels.push(json!({"channel_id": id, "members": [{"service": {"rid": "svc-lamp", "rtype": "entertainment"}, "index": 0}]}));
        }
    }
    let provider = connected(&bridge).await;
    let error = output(&provider).start(LAMP).await.unwrap_err();
    assert!(matches!(error, ProviderError::Refused(m) if m.contains("20")));
    assert!(!bridge.requests().iter().any(|r| r.starts_with("PUT")));
}

#[tokio::test]
async fn another_apps_stream_is_reported_and_taken_over_only_on_request() {
    let bridge = FakeBridge::start().await;
    bridge.stream_from_another_app();
    let provider = connected(&bridge).await;
    let out = output(&provider);

    let error = out.start(LAMP).await.unwrap_err();
    assert!(
        matches!(&error, ProviderError::Refused(m) if m.starts_with(folo_provider_hue::IN_USE_BY_ANOTHER_APP)),
        "{error}"
    );
    assert!(
        !bridge.requests().iter().any(|r| r.starts_with("PUT")),
        "the stream was not touched"
    );
    assert_eq!(bridge.dtls.observed.lock().unwrap().handshakes, 0);
    let areas = provider
        .admin("entertainment", AdminMethod::Read, Value::Null)
        .await
        .unwrap();
    assert_eq!(areas["areas"][0]["in_use_by_another_app"], true);

    // Asking what taking over would do changes nothing
    let preview = provider
        .admin("entertainment_take_over", AdminMethod::Read, Value::Null)
        .await
        .unwrap();
    assert!(
        preview["would"]
            .as_str()
            .unwrap()
            .contains("Stops the stream of another app")
    );
    assert!(!bridge.requests().iter().any(|r| r.starts_with("PUT")));
    assert!(out.start(LAMP).await.is_err());

    // Only the explicit request stops their stream, and then start works
    let done = provider
        .admin("entertainment_take_over", AdminMethod::Change, Value::Null)
        .await
        .unwrap();
    assert_eq!(done["stopped"][0]["id"], AREA);
    out.start(LAMP).await.unwrap();
    assert_eq!(
        bridge.area(AREA)["active_streamer"]["rid"],
        support::fake_bridge::APPLICATION_ID
    );
}

#[tokio::test]
async fn our_own_stale_stream_is_stopped_and_started_again() {
    let bridge = FakeBridge::start().await;
    {
        let mut state = bridge.state();
        state.configurations[0]["status"] = json!("active");
        state.configurations[0]["active_streamer"] =
            json!({"rid": support::fake_bridge::APPLICATION_ID, "rtype": "auth_v1"});
    }
    let provider = connected(&bridge).await;
    output(&provider).start(LAMP).await.unwrap();
    let bodies: Vec<Value> = bridge
        .state()
        .requests
        .iter()
        .filter(|r| r.0 == "PUT")
        .map(|r| r.2.clone())
        .collect();
    assert_eq!(
        bodies,
        vec![json!({"action": "stop"}), json!({"action": "start"})]
    );
}

#[tokio::test]
async fn a_lost_stream_is_reported_so_the_sender_reconnects_and_stops_the_area_then() {
    let bridge = FakeBridge::start().await;
    bridge
        .dtls
        .close_after
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let provider = connected(&bridge).await;
    let out = output(&provider);
    out.start(LAMP).await.unwrap();
    out.send(&frame(vec![lamp(1.0, 1.0, 1.0)])).await.unwrap();
    let error = loop {
        match out.send(&frame(vec![lamp(2.0, 2.0, 2.0)])).await {
            Err(e) => break e,
            Ok(()) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    assert!(matches!(error, ProviderError::Unreachable(_)), "{error}");

    // The connection starts over: the area the dead stream left is stopped
    provider.connect().await.unwrap();
    assert_eq!(bridge.area(AREA)["status"], "inactive");
}

#[tokio::test]
async fn the_sender_drives_the_bridge_end_to_end_with_keepalives_and_a_clean_stop() {
    let bridge = FakeBridge::start().await;
    let provider = support::provider(&bridge);
    let mut policy = provider.delivery_policy();
    policy.keepalive = Duration::from_millis(150);
    let config = SenderConfig {
        rate_hz: 50.0,
        targets: vec![
            SenderTarget {
                id: LAMP.into(),
                delivery: Delivery::Streaming,
            },
            SenderTarget {
                id: STRIP.into(),
                delivery: Delivery::Streaming,
            },
        ],
        policy,
        backoff: folo_provider::Backoff::default(),
        start_retry: Duration::from_millis(200),
        describe_every: None,
    };
    let sender = spawn_sender(Arc::clone(&provider), config).unwrap();
    let mut status = sender.status();
    sender.submit(frame(vec![lamp(255.0, 0.0, 0.0), strip([[0.0; 3]; 3])]));
    tokio::time::timeout(
        Duration::from_secs(10),
        status.wait_for(|s| s.state == ProviderState::Connected),
    )
    .await
    .unwrap()
    .unwrap();
    bridge
        .dtls
        .eventually("the first packet", |o| !o.datagrams.is_empty())
        .await;
    // Nothing changes, so only keepalives follow
    bridge
        .dtls
        .eventually("keepalives", |o| o.datagrams.len() >= 4)
        .await;
    let datagrams = bridge.dtls.datagrams();
    assert!(datagrams.iter().all(|d| d.len() == 52 + 4 * 7));
    assert_eq!(
        &datagrams[0][52..59],
        &hex::decode("00ffff00000000").unwrap()[..]
    );

    sender.submit(frame(Vec::new()));
    bridge
        .dtls
        .eventually("close_notify", |o| o.closes == 1)
        .await;
    sender.shutdown().await;
    assert_eq!(bridge.area(AREA)["status"], "inactive");
}

#[tokio::test]
async fn nothing_secret_is_logged_while_connecting_and_streaming() {
    support::capture_logs();
    let bridge = FakeBridge::start().await;
    bridge.stream_from_another_app();
    let provider = connected(&bridge).await;
    let _ = output(&provider).start(LAMP).await;
    let _ = provider.connect().await;
    provider.disconnect().await;
    let log = support::logged().join("\n");
    for secret in [USERNAME, CLIENT_KEY, &CLIENT_KEY.to_uppercase()] {
        assert!(!log.contains(secret), "a secret reached the log: {log}");
    }
    let _ = BRIDGE_ID;
    let _ = HueKind::default();
    let _: &dyn ProviderKind = &HueKind::default();
}

#[tokio::test]
async fn another_apps_stream_is_never_stopped_by_a_release_or_a_reconnect() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let out = output(&provider);
    out.start(LAMP).await.unwrap();
    // Another app takes the area over while ours runs
    bridge.stream_from_another_app();
    out.release(LAMP).await.unwrap();
    assert_eq!(
        bridge.area(AREA)["status"],
        "active",
        "their stream runs on"
    );
    let stops = |bridge: &FakeBridge| {
        bridge
            .state()
            .requests
            .iter()
            .filter(|r| r.2 == json!({"action": "stop"}))
            .count()
    };
    assert_eq!(stops(&bridge), 0);

    // The same after a stream that failed: the area left to stop is theirs by the time we reconnect
    bridge.state().configurations[0]["status"] = json!("inactive");
    bridge.state().configurations[0]
        .as_object_mut()
        .unwrap()
        .remove("active_streamer");
    bridge
        .dtls
        .close_after
        .store(1, std::sync::atomic::Ordering::SeqCst);
    out.start(LAMP).await.unwrap();
    out.send(&frame(vec![lamp(1.0, 1.0, 1.0)])).await.unwrap();
    while out.send(&frame(vec![lamp(2.0, 2.0, 2.0)])).await.is_ok() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bridge.stream_from_another_app();
    let before = stops(&bridge);
    provider.connect().await.unwrap();
    assert_eq!(stops(&bridge), before);
    assert_eq!(bridge.area(AREA)["status"], "active");
}

#[tokio::test]
async fn a_released_target_goes_dark_while_the_stream_runs_for_the_others() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let out = output(&provider);
    out.start(LAMP).await.unwrap();
    out.start(STRIP).await.unwrap();
    out.send(&frame(vec![
        lamp(9.0, 9.0, 9.0),
        strip([[5.0; 3], [6.0; 3], [7.0; 3]]),
    ]))
    .await
    .unwrap();
    out.release(LAMP).await.unwrap();
    // The strip changes, so a packet goes out; the lamp's channel is black in it
    out.send(&frame(vec![strip([[5.0; 3], [6.0; 3], [8.0; 3]])]))
        .await
        .unwrap();
    bridge
        .dtls
        .eventually("two datagrams", |o| o.datagrams.len() == 2)
        .await;
    let expected = [
        record(0, 0, 0, 0),
        record(1, 5, 5, 5),
        record(2, 6, 6, 6),
        record(3, 8, 8, 8),
    ]
    .concat();
    assert_eq!(bridge.dtls.datagrams()[1], golden(1, &expected));
}
