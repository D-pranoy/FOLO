//! Whether a bridge is believed: Signify's roots, the common name, trust on first use, and
//! discovery (never reaching the internet).

mod support;

use std::sync::Arc;

use folo_provider::{ProviderError, ProviderKind};
use folo_provider_hue::bridge::{ProbeTrust, client_for, probe};
use folo_provider_hue::discovery::{DiscoveryConfig, discover};
use folo_provider_hue::endpoint::Endpoint;
use folo_provider_hue::rest::RestLimits;
use folo_provider_hue::trust::{
    Anchors, BridgeTrust, MemoryPins, PinStore, TrustError, fingerprint_of, public_config,
};
use folo_provider_hue::{HueKind, HueOptions};
use support::fake_bridge::{BRIDGE_ID, Certificate, FakeBridge};

fn test_anchors(bridge: &FakeBridge) -> Anchors {
    Anchors::Custom(vec![bridge.authority_der.clone()])
}

#[tokio::test]
async fn a_bridge_signed_by_a_trusted_root_and_named_for_it_is_trusted() {
    let bridge = FakeBridge::start().await;
    let probe = probe(
        &bridge.endpoint,
        &test_anchors(&bridge),
        RestLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(probe.trust, ProbeTrust::Trusted);
    assert!(probe.info.bridge_id.eq_ignore_ascii_case(BRIDGE_ID));
    assert_eq!(probe.certificate.subject, BRIDGE_ID);
}

#[tokio::test]
async fn the_bundled_signify_roots_do_not_vouch_for_a_test_authority() {
    let bridge = FakeBridge::start().await;
    let probe = probe(&bridge.endpoint, &Anchors::Signify, RestLimits::default())
        .await
        .unwrap();
    assert!(
        matches!(
            probe.trust,
            ProbeTrust::NeedsConfirmation(TrustError::Chain(_))
        ),
        "{:?}",
        probe.trust
    );
}

#[tokio::test]
async fn a_certificate_for_another_bridge_is_rejected_not_offered() {
    let bridge = FakeBridge::with_certificate(Certificate::WrongName).await;
    let probe = probe(
        &bridge.endpoint,
        &test_anchors(&bridge),
        RestLimits::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(
            probe.trust,
            ProbeTrust::Rejected(TrustError::NameMismatch { .. })
        ),
        "{:?}",
        probe.trust
    );
}

#[tokio::test]
async fn a_client_refuses_a_bridge_it_does_not_believe() {
    let bridge = FakeBridge::with_certificate(Certificate::WrongName).await;
    let trust = BridgeTrust {
        bridge_id: BRIDGE_ID.into(),
        anchors: test_anchors(&bridge),
        pinned: None,
    };
    let client = client_for(&bridge.endpoint, trust, None, RestLimits::default()).unwrap();
    assert!(client.public_config().await.is_err());
    assert!(
        bridge.requests().is_empty(),
        "no request reached the bridge: {:?}",
        bridge.requests()
    );

    let signify = BridgeTrust::signify(BRIDGE_ID);
    let client = client_for(&bridge.endpoint, signify, None, RestLimits::default()).unwrap();
    assert!(client.public_config().await.is_err());
}

#[tokio::test]
async fn a_self_signed_bridge_is_trusted_only_once_its_fingerprint_is_pinned() {
    let bridge = FakeBridge::with_certificate(Certificate::SelfSigned).await;
    let seen = probe(&bridge.endpoint, &Anchors::Signify, RestLimits::default())
        .await
        .unwrap();
    assert!(matches!(seen.trust, ProbeTrust::NeedsConfirmation(_)));
    assert_eq!(
        seen.certificate.fingerprint,
        fingerprint_of(&bridge.certificate_der).unwrap()
    );

    let unpinned = BridgeTrust {
        bridge_id: BRIDGE_ID.into(),
        anchors: Anchors::Signify,
        pinned: None,
    };
    let client = client_for(&bridge.endpoint, unpinned, None, RestLimits::default()).unwrap();
    assert!(
        client.public_config().await.is_err(),
        "not trusted before it was confirmed"
    );

    let pinned = BridgeTrust {
        pinned: Some(seen.certificate.fingerprint),
        ..BridgeTrust::signify(BRIDGE_ID)
    };
    let client = client_for(&bridge.endpoint, pinned, None, RestLimits::default()).unwrap();
    assert!(client.public_config().await.is_ok());

    // Another certificate for the same bridge is not the one that was confirmed
    let other = FakeBridge::with_certificate(Certificate::SelfSigned).await;
    let client = client_for(
        &other.endpoint,
        BridgeTrust {
            pinned: Some(seen.certificate.fingerprint),
            ..BridgeTrust::signify(BRIDGE_ID)
        },
        None,
        RestLimits::default(),
    )
    .unwrap();
    assert!(client.public_config().await.is_err());
}

#[tokio::test]
async fn an_instance_connects_to_a_self_signed_bridge_after_the_pin_is_stored() {
    let bridge = FakeBridge::with_certificate(Certificate::SelfSigned).await;
    let pins = Arc::new(MemoryPins::default());
    let mut options = support::options(&bridge);
    options.anchors = Anchors::Signify;
    options.pins = pins.clone();
    let kind = HueKind::new(options);
    let instance = support::instance(&bridge, "hue1");
    let provider = kind.create(&instance, Some(&support::secrets())).unwrap();
    assert!(matches!(
        provider.connect().await,
        Err(ProviderError::Unreachable(_))
    ));

    kind.pin_certificate(BRIDGE_ID, fingerprint_of(&bridge.certificate_der).unwrap());
    assert!(pins.pinned(BRIDGE_ID).is_some());
    provider.connect().await.unwrap();
}

#[tokio::test]
async fn discovery_asks_the_injected_service_only_and_only_when_mdns_finds_nothing() {
    let bridge = FakeBridge::start().await;
    let mut config = DiscoveryConfig {
        mdns_timeout: std::time::Duration::from_millis(50),
        cloud_url: Some(format!("{}/discovery", bridge.endpoint.base_url())),
        cloud_roots: vec![bridge.authority_der.clone()],
        ..DiscoveryConfig::default()
    };
    let found = discover(&config).await;
    // mDNS may or may not find real bridges on the machine running the tests; there are none
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].bridge_id, BRIDGE_ID);
    assert!(
        bridge.requests().is_empty(),
        "discovery only listed bridges: {:?}",
        bridge.requests()
    );

    config.cloud_url = None;
    assert_eq!(discover(&config).await, Vec::new());
}

#[tokio::test]
async fn the_kind_lists_discovered_bridges_for_the_client_api() {
    let bridge = FakeBridge::start().await;
    let mut options: HueOptions = support::options(&bridge);
    options.discovery.cloud_url = Some(format!("{}/discovery", bridge.endpoint.base_url()));
    options.discovery.cloud_roots = vec![bridge.authority_der.clone()];
    let devices = HueKind::new(options).discover().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(&**devices[0].id, BRIDGE_ID);
    assert_eq!(
        devices[0].kind,
        folo_protocol::generated::DiscoveredDeviceKind::Hue
    );
}

#[test]
fn the_public_roots_build_without_the_internet() {
    assert!(public_config(&[]).is_ok());
    let _ = Endpoint::new("127.0.0.1");
}
