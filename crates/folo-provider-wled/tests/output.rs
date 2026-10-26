//! The provider's output over loopback: the fake device's HTTP API and its UDP receivers, and on
//! a paused clock the sender's refresh while static and the release.

use std::sync::Arc;
use std::time::Duration;

use folo_protocol::generated::{ProviderInstance, ProviderSecrets};
use folo_provider::{
    Backoff, ElementColors, ElementFrame, ElementOutput, Provider, ProviderError, ProviderKind,
    ProviderState, SenderConfig, SenderTarget, TargetOutput, spawn_sender,
};
use folo_provider_wled::dnrgb::DNRGB_PORT;
use folo_provider_wled::testing::{
    DdpFrames, DdpPacket, DnrgbPacket, FakeDevice, FakeHttpServer, FakeUdpReceiver, MemorySink,
};
use folo_provider_wled::{Config, Transport, WledKind, WledProvider};
use serde_json::json;
use tokio::time::sleep;

fn frame(outputs: Vec<(&str, ElementColors)>) -> ElementFrame {
    ElementFrame::new(
        0,
        outputs
            .into_iter()
            .map(|(target, colors)| TargetOutput {
                target: target.to_owned(),
                colors,
            })
            .collect(),
    )
}

fn red(count: usize, level: f32) -> ElementColors {
    ElementColors::Rgb(vec![[level, 0.0, 0.0]; count])
}

/// A device on loopback: its HTTP API and the two UDP receivers, and the provider of an instance
/// made by the kind, as the node makes it.
struct Rig {
    _http: FakeHttpServer,
    ddp: FakeUdpReceiver,
    dnrgb: FakeUdpReceiver,
    provider: Arc<dyn Provider>,
}

async fn rig(fixture: &str, transport: &str, timeout_s: u64) -> Rig {
    let device = Arc::new(FakeDevice::fixture(fixture));
    let http = FakeHttpServer::start(device.clone()).await;
    let ddp = FakeUdpReceiver::bind().await;
    let dnrgb = FakeUdpReceiver::bind().await;
    let instance: ProviderInstance = serde_json::from_value(json!({
        "id": "wled1", "kind": "wled", "name": "Test",
        "settings": {"kind": "wled", "host": http.authority(), "transport": transport,
                     "timeout_s": timeout_s},
    }))
    .unwrap();
    let kind = WledKind::new().with_ports(ddp.port(), dnrgb.port());
    let provider = kind.create(&instance, None).unwrap();
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();
    Rig {
        _http: http,
        ddp,
        dnrgb,
        provider,
    }
}

fn output(rig: &Rig) -> &dyn ElementOutput {
    rig.provider.element_output().unwrap()
}

#[tokio::test]
async fn ddp_frames_reach_the_receiver_with_exact_bytes() {
    let rig = rig("rgb-strip", "ddp", 2).await;
    let out = output(&rig);
    out.start("strip").await.unwrap();
    out.send(&frame(vec![("strip", red(4, 255.9))]))
        .await
        .unwrap();

    let datagrams = rig.ddp.wait_for(1).await;
    let mut expected = vec![0x41, 0x01, 0x0B, 0x01, 0, 0, 0, 0, 0x00, 0x0C];
    expected.extend([255, 0, 0].repeat(4));
    assert_eq!(
        datagrams,
        vec![expected],
        "levels are truncated, not rounded"
    );
}

#[tokio::test]
async fn a_long_frame_is_split_and_rendered_once() {
    let rig = rig("rgb-strip", "ddp", 2).await;
    let out = output(&rig);
    out.start("strip").await.unwrap();
    // 60 LEDs only; the strip target clips what is beyond it
    out.send(&frame(vec![("strip", red(600, 10.0))]))
        .await
        .unwrap();
    let datagrams = rig.ddp.wait_for(1).await;
    assert_eq!(datagrams.len(), 1, "60 pixels are one packet");
    assert_eq!(datagrams[0].len(), 10 + 180);
}

#[tokio::test]
async fn a_segment_is_addressed_by_its_absolute_range() {
    let rig = rig("segments", "ddp", 2).await;
    let out = output(&rig);
    out.start("seg:1").await.unwrap();
    out.send(&frame(vec![("seg:1", red(2, 7.0))]))
        .await
        .unwrap();
    let packet = DdpPacket::parse(&rig.ddp.wait_for(1).await[0]).unwrap();
    // The middle segment starts at LED 30: 30 * 3 bytes
    assert_eq!(packet.offset, 90);
    assert_eq!(packet.data, [7, 0, 0, 7, 0, 0]);
    assert!(packet.push());
}

#[tokio::test]
async fn main_segment_only_makes_indices_relative_to_the_main_segment() {
    let rig = rig("main-segment-only", "ddp", 2).await;
    let out = output(&rig);
    out.start("seg:1").await.unwrap();
    out.send(&frame(vec![("seg:1", red(3, 9.0))]))
        .await
        .unwrap();
    let packet = DdpPacket::parse(&rig.ddp.wait_for(1).await[0]).unwrap();
    assert_eq!(
        packet.offset, 0,
        "the main segment starts at LED 20, and index 0 is that LED"
    );
}

#[tokio::test]
async fn rgbw_targets_send_the_white_byte() {
    let rig = rig("rgbw-strip", "ddp", 2).await;
    let out = output(&rig);
    out.start("strip").await.unwrap();
    out.send(&frame(vec![(
        "strip",
        ElementColors::Rgbw(vec![[1.0, 2.0, 3.0, 4.5], [5.0, 6.0, 7.0, 8.0]]),
    )]))
    .await
    .unwrap();
    let packet = DdpPacket::parse(&rig.ddp.wait_for(1).await[0]).unwrap();
    assert_eq!(packet.data_type, 0x1B);
    assert_eq!(packet.channels(), 4);
    assert_eq!(packet.data, [1, 2, 3, 4, 5, 6, 7, 8]);
}

#[tokio::test]
async fn white_is_left_out_when_the_auto_white_mode_would_ignore_it() {
    let rig = rig("rgbw-auto-white", "ddp", 2).await;
    let out = output(&rig);
    out.start("strip").await.unwrap();
    out.send(&frame(vec![(
        "strip",
        ElementColors::Rgbw(vec![[1.0, 2.0, 3.0, 4.0]]),
    )]))
    .await
    .unwrap();
    let packet = DdpPacket::parse(&rig.ddp.wait_for(1).await[0]).unwrap();
    assert_eq!(packet.data_type, 0x0B);
    assert_eq!(packet.data, [1, 2, 3]);
}

#[tokio::test]
async fn point_targets_convert_their_models_to_pixels() {
    let rig = rig("analog-and-relay", "ddp", 2).await;
    let out = output(&rig);
    for target in ["bus:1", "bus:2", "bus:3"] {
        out.start(target).await.unwrap();
    }
    // bus:2 is a relay without white: on is grey at full; bus:3 is a white PWM output whose
    // auto-white mode (manual) takes our white; bus:1 is RGB+CCT, so its white goes out and its
    // kelvin is dropped
    out.send(&frame(vec![
        ("bus:2", ElementColors::OnOff(vec![true])),
        ("bus:3", ElementColors::Dimmer(vec![40.0])),
        (
            "bus:1",
            ElementColors::RgbCct(vec![folo_provider::RgbCct {
                rgb: [1.0, 2.0, 3.0],
                white: 4.0,
                kelvin: 3000.0,
            }]),
        ),
    ]))
    .await
    .unwrap();
    let packets: Vec<DdpPacket> = rig
        .ddp
        .wait_for(3)
        .await
        .iter()
        .map(|d| DdpPacket::parse(d).unwrap())
        .collect();
    // The relay is LED 31, as RGB: byte offset 93
    assert_eq!(
        (packets[0].offset, packets[0].data.clone()),
        (93, vec![255, 255, 255])
    );
    // The white output is LED 32, as RGBW: byte offset 128, type 0x1B
    assert_eq!((packets[1].offset, packets[1].data_type), (128, 0x1B));
    assert_eq!(packets[1].data, [0, 0, 0, 40]);
    // The RGB+CCT output is LED 30, as RGBW: byte offset 120
    assert_eq!(
        (packets[2].offset, packets[2].data.clone()),
        (120, vec![1, 2, 3, 4])
    );
    assert!(!packets[0].push() && !packets[1].push() && packets[2].push());
}

#[tokio::test]
async fn dnrgb_frames_carry_the_timeout_and_the_start_index() {
    let rig = rig("segments", "dnrgb", 5).await;
    let out = output(&rig);
    out.start("seg:2").await.unwrap();
    out.send(&frame(vec![("seg:2", red(3, 20.0))]))
        .await
        .unwrap();
    let packet = DnrgbPacket::parse(&rig.dnrgb.wait_for(1).await[0]).unwrap();
    assert_eq!(packet.protocol, 4);
    assert_eq!(packet.timeout_s, 5);
    assert_eq!(packet.start, 60);
    assert_eq!(packet.data, [20, 0, 0, 20, 0, 0, 20, 0, 0]);
    assert_eq!(rig.ddp.datagrams().len(), 0);
}

#[tokio::test]
async fn release_waits_for_the_last_target_and_gives_the_lights_back_with_timeout_zero() {
    let rig = rig("segments", "ddp", 2).await;
    let out = output(&rig);
    out.start("seg:0").await.unwrap();
    out.start("seg:1").await.unwrap();
    out.start("seg:1").await.unwrap(); // after a reconnect, a repeat is harmless
    out.send(&frame(vec![("seg:0", red(2, 1.0))]))
        .await
        .unwrap();

    out.release("seg:0").await.unwrap();
    sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.dnrgb.datagrams().len(), 0, "seg:1 still runs");

    out.release("seg:1").await.unwrap();
    let release = rig.dnrgb.wait_for(1).await;
    assert_eq!(release, vec![vec![4, 0, 0, 0]]);
    assert!(DnrgbPacket::parse(&release[0]).unwrap().releases());

    // A target that isn't running is released harmlessly
    out.release("seg:2").await.unwrap();
    sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.dnrgb.datagrams().len(), 1);
}

#[tokio::test]
async fn frames_are_rendered_on_push_only() {
    let rig = rig("rgb-strip", "ddp", 2).await;
    let out = output(&rig);
    out.start("strip").await.unwrap();
    out.send(&frame(vec![("strip", red(60, 3.0))]))
        .await
        .unwrap();
    out.send(&frame(vec![("strip", red(60, 4.0))]))
        .await
        .unwrap();
    let mut frames = DdpFrames::default();
    for datagram in rig.ddp.wait_for(2).await {
        frames.apply(&DdpPacket::parse(&datagram).unwrap());
    }
    assert_eq!(frames.rendered.len(), 2);
    assert_eq!(frames.rendered[1][0], 4);
    // Sequence numbers kept counting between frames
    let sequences: Vec<u8> = rig.ddp.datagrams().iter().map(|d| d[1]).collect();
    assert_eq!(sequences, [1, 2]);
}

#[tokio::test]
async fn starting_an_unknown_target_is_refused_and_sending_before_describe_fails() {
    let rig = rig("rgb-strip", "ddp", 2).await;
    let error = output(&rig).start("seg:9").await.unwrap_err();
    assert!(matches!(error, ProviderError::Refused(_)));

    let device = Arc::new(FakeDevice::fixture("rgb-strip"));
    let config = Config {
        authority: "wled.test".into(),
        host: "wled.test".into(),
        transport: Transport::Ddp,
        timeout_s: 2,
        pin: None,
        ddp_port: 4048,
        dnrgb_port: None,
    };
    let provider = WledProvider::new(config, device, Arc::new(MemorySink::new()));
    let error = provider
        .send(&frame(vec![("strip", red(1, 1.0))]))
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Failed(_)));
}

#[tokio::test]
async fn the_wire_key_is_the_frame_as_the_device_gets_it() {
    let rig = rig("rgb-strip", "ddp", 2).await;
    let out = output(&rig);
    let key = |level: f32| {
        out.wire_key(&TargetOutput {
            target: "strip".into(),
            colors: red(3, level),
        })
    };
    assert_eq!(key(10.2), key(10.9), "the device can't tell them apart");
    assert_ne!(key(10.0), key(11.0));
    assert_eq!(key(10.0), [10, 0, 0, 10, 0, 0, 10, 0, 0]);
}

#[tokio::test]
async fn the_pin_comes_from_the_instances_secrets() {
    let device = Arc::new(FakeDevice::fixture("rgbw-strip").with_pin("4321"));
    let http = FakeHttpServer::start(device.clone()).await;
    let instance: ProviderInstance = serde_json::from_value(json!({
        "id": "w", "kind": "wled",
        "settings": {"kind": "wled", "host": http.authority(), "transport": "ddp", "timeout_s": 2},
    }))
    .unwrap();
    let secrets: ProviderSecrets = serde_json::from_value(json!({"pin": "4321"})).unwrap();
    let provider = WledKind::new().create(&instance, Some(&secrets)).unwrap();
    let targets = provider.describe().await.unwrap();
    assert_eq!(
        targets[0].capabilities.color_model,
        folo_protocol::generated::CapabilityDescriptorColorModel::Rgbw
    );
}

#[tokio::test]
async fn an_instance_of_another_kind_or_without_a_host_is_refused() {
    let hyperion: ProviderInstance =
        serde_json::from_value(json!({"id": "h", "kind": "hyperion"})).unwrap();
    assert!(WledKind::new().create(&hyperion, None).is_err());
    let no_host: ProviderInstance = serde_json::from_value(json!({
        "id": "w", "kind": "wled",
        "settings": {"kind": "wled", "host": "", "transport": "ddp", "timeout_s": 2}
    }))
    .unwrap();
    assert!(WledKind::new().create(&no_host, None).is_err());
}

#[tokio::test]
async fn connect_reaches_the_device_and_a_dead_one_is_unreachable() {
    let device = Arc::new(FakeDevice::fixture("rgb-strip"));
    let sink = Arc::new(MemorySink::new());
    let config = Config {
        authority: "wled.test".into(),
        host: "wled.test".into(),
        transport: Transport::Ddp,
        timeout_s: 2,
        pin: None,
        ddp_port: 4048,
        dnrgb_port: None,
    };
    let provider = WledProvider::new(config, device, sink.clone());
    provider.connect().await.unwrap();
    assert_eq!(
        sink.resets(),
        1,
        "the address is looked up again after a reconnect"
    );

    // Nothing listens on a port we just freed
    let freed = FakeHttpServer::start(Arc::new(FakeDevice::fixture("rgb-strip"))).await;
    let authority = freed.authority();
    drop(freed);
    sleep(Duration::from_millis(50)).await;
    let instance: ProviderInstance = serde_json::from_value(json!({
        "id": "w", "kind": "wled",
        "settings": {"kind": "wled", "host": authority, "transport": "ddp", "timeout_s": 2}
    }))
    .unwrap();
    let provider = WledKind::new().create(&instance, None).unwrap();
    let error = provider.connect().await.unwrap_err();
    assert!(matches!(error, ProviderError::Unreachable(_)), "{error}");
}

// The sender on a paused clock, with the datagrams kept in memory

fn sender_config(provider: &Arc<WledProvider>) -> SenderConfig {
    SenderConfig {
        rate_hz: 60.0,
        targets: vec![SenderTarget {
            id: "strip".into(),
            delivery: folo_provider::Delivery::Streaming,
        }],
        policy: provider.delivery_policy(),
        backoff: Backoff::default(),
        start_retry: Duration::from_secs(5),
        describe_every: None,
    }
}

fn memory_provider(fixture: &str) -> (Arc<WledProvider>, Arc<MemorySink>) {
    let device = Arc::new(FakeDevice::fixture(fixture));
    let sink = Arc::new(MemorySink::new());
    let config = Config {
        authority: "wled.test".into(),
        host: "wled.test".into(),
        transport: Transport::Ddp,
        timeout_s: 2,
        pin: None,
        ddp_port: 4048,
        dnrgb_port: None,
    };
    (
        Arc::new(WledProvider::new(config, device, sink.clone())),
        sink,
    )
}

#[tokio::test(start_paused = true)]
async fn a_static_output_is_refreshed_every_second() {
    let (provider, sink) = memory_provider("rgb-strip");
    let sender = spawn_sender(provider.clone(), sender_config(&provider)).unwrap();
    sender.submit(frame(vec![("strip", red(60, 50.0))]));

    sleep(Duration::from_millis(3500)).await;
    let ddp: Vec<_> = sink
        .timed()
        .into_iter()
        .filter(|(_, port, _)| *port == 4048)
        .collect();
    assert!(
        (4..=5).contains(&ddp.len()),
        "one at once, then one a second: {}",
        ddp.len()
    );
    for pair in ddp.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        assert!(
            gap >= Duration::from_secs(1) && gap < Duration::from_millis(1100),
            "refreshed after {gap:?}"
        );
        // The same pixels, with the next sequence number
        assert_eq!(pair[0].2[10..], pair[1].2[10..]);
    }
    assert!(ddp.iter().all(|(_, _, p)| p[0] == 0x41));
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_changed_frame_goes_out_at_once_and_the_stop_releases() {
    let (provider, sink) = memory_provider("rgb-strip");
    let sender = spawn_sender(provider.clone(), sender_config(&provider)).unwrap();
    sender.submit(frame(vec![("strip", red(60, 50.0))]));
    sleep(Duration::from_millis(100)).await;
    assert_eq!(sink.sent().len(), 1);

    sender.submit(frame(vec![("strip", red(60, 51.0))]));
    sleep(Duration::from_millis(100)).await;
    assert_eq!(sink.sent().len(), 2, "within a few ticks, not a second");

    let mut status = sender.status();
    assert_eq!(status.borrow_and_update().state, ProviderState::Connected);

    sender.shutdown().await;
    let sent = sink.sent();
    // The last datagram is the release, on the UDP port the device reports, with timeout 0
    assert_eq!(sent.last().unwrap(), &(DNRGB_PORT, vec![4, 0, 0, 0]));
    assert_eq!(sent.len(), 3);
}

#[tokio::test]
async fn a_single_pixel_target_goes_by_ddp_even_when_the_frame_uses_dnrgb() {
    let rig = rig("analog-and-relay", "dnrgb", 2).await;
    let out = output(&rig);
    for target in ["strip", "bus:2"] {
        out.start(target).await.unwrap();
    }
    out.send(&frame(vec![
        ("strip", red(4, 5.0)),
        ("bus:2", ElementColors::OnOff(vec![true])),
    ]))
    .await
    .unwrap();
    let strip = DnrgbPacket::parse(&rig.dnrgb.wait_for(1).await[0]).unwrap();
    assert_eq!((strip.start, strip.data.len()), (0, 12));
    let relay = DdpPacket::parse(&rig.ddp.wait_for(1).await[0]).unwrap();
    assert_eq!(
        (relay.offset, relay.data.clone()),
        (93, vec![255, 255, 255])
    );
    assert!(relay.push());
}

#[test]
fn the_keepalive_stays_inside_the_realtime_timeout() {
    let keepalive = |timeout_s: u64| {
        let config = Config {
            authority: "wled.test".into(),
            host: "wled.test".into(),
            transport: Transport::Ddp,
            timeout_s,
            pin: None,
            ddp_port: 4048,
            dnrgb_port: None,
        };
        let device = Arc::new(FakeDevice::fixture("rgb-strip"));
        WledProvider::new(config, device, Arc::new(MemorySink::new()))
            .delivery_policy()
            .keepalive
    };
    assert_eq!(keepalive(2), Duration::from_secs(1));
    assert_eq!(keepalive(10), Duration::from_secs(1));
    assert_eq!(keepalive(1), Duration::from_millis(500));
}

#[test]
fn new_instances_run_at_60_hz() {
    assert_eq!(folo_provider_wled::DEFAULT_RATE_HZ, 60);
}
