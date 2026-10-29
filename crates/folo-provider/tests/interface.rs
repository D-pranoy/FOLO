//! `effective_capabilities`, the registry, the state mapping and the recording provider.

use std::sync::Arc;

use folo_protocol::generated::{
    Capabilities, CapabilityDescriptorAddressing, CapabilityDescriptorColorModel,
    CapabilityDescriptorDelivery, DiscoveredDevice, ProviderInstance, ProviderInstanceKind,
    ProviderLiveState, ProviderStatusState,
};
use folo_provider::testing::{Event, RecordingKind, RecordingProvider};
use folo_provider::{
    AdminError, AdminMethod, CreateError, ElementColors, Provider, ProviderError, ProviderState,
    Registry, TargetOutput, effective_capabilities, not_implemented_message, runbook,
};
use serde_json::json;

fn capabilities(value: serde_json::Value) -> Capabilities {
    serde_json::from_value(value).unwrap()
}

fn device(kind: &str, id: &str) -> DiscoveredDevice {
    serde_json::from_value(json!({"kind": kind, "id": id, "name": id, "address": "host"})).unwrap()
}

fn instance(kind: &str) -> ProviderInstance {
    serde_json::from_value(json!({"id": "i", "kind": kind})).unwrap()
}

#[test]
fn without_overrides_the_discovered_descriptor_stands() {
    let caps = capabilities(json!({
        "discovered": {"color_model": "rgbw", "delivery": "state", "addressing": "zoned",
                       "limits": {"max_rate_hz": 12, "latency_ms": 80}}
    }));
    let effective = effective_capabilities(&caps);
    assert_eq!(effective.color_model, CapabilityDescriptorColorModel::Rgbw);
    assert_eq!(effective.delivery, CapabilityDescriptorDelivery::State);
    assert_eq!(effective.addressing, CapabilityDescriptorAddressing::Zoned);
    assert!((effective.limits.max_rate_hz - 12.0).abs() < 1e-9);
    assert!((effective.limits.latency_ms - 80.0).abs() < 1e-9);
}

#[test]
fn overrides_win_field_by_field() {
    let caps = capabilities(json!({
        "discovered": {"color_model": "rgb", "delivery": "streaming", "addressing": "pixel"},
        "overrides": {"color_model": "dimmer", "delivery": "state"}
    }));
    let effective = effective_capabilities(&caps);
    assert_eq!(
        effective.color_model,
        CapabilityDescriptorColorModel::Dimmer
    );
    assert_eq!(effective.delivery, CapabilityDescriptorDelivery::State);
    // Not overridden: discovered
    assert_eq!(effective.addressing, CapabilityDescriptorAddressing::Pixel);
}

#[test]
fn an_override_of_the_limits_replaces_them() {
    let caps = capabilities(json!({
        "discovered": {"limits": {"max_rate_hz": 25, "latency_ms": 100}},
        "overrides": {"limits": {"max_rate_hz": 60}}
    }));
    let effective = effective_capabilities(&caps);
    assert!((effective.limits.max_rate_hz - 60.0).abs() < 1e-9);
    // The contract gives an override's missing limits their defaults, which replace the
    // discovered ones
    assert!(effective.limits.latency_ms.abs() < 1e-9);
}

#[test]
fn defaults_are_the_default_descriptor() {
    let effective = effective_capabilities(&Capabilities::default());
    assert_eq!(effective.color_model, CapabilityDescriptorColorModel::Rgb);
    assert_eq!(effective.delivery, CapabilityDescriptorDelivery::Streaming);
}

#[test]
fn states_map_to_both_lists() {
    use ProviderState::{Connected, Connecting, Error, Stopped};
    assert_eq!(
        [Stopped, Connecting, Connected, Error].map(ProviderState::api),
        [
            ProviderStatusState::Stopped,
            ProviderStatusState::Connecting,
            ProviderStatusState::Connected,
            ProviderStatusState::Error
        ]
    );
    assert_eq!(
        [Stopped, Connecting, Connected, Error].map(ProviderState::live),
        [
            ProviderLiveState::Off,
            ProviderLiveState::Disconnected,
            ProviderLiveState::Connected,
            ProviderLiveState::Error
        ]
    );
}

#[test]
fn run_books_name_the_kinds_that_arrive() {
    assert_eq!(runbook(ProviderInstanceKind::Hyperion), "RB-10");
    assert_eq!(runbook(ProviderInstanceKind::Wled), "RB-12");
    assert_eq!(
        not_implemented_message(ProviderInstanceKind::Hue),
        "arrives with RB-13"
    );
}

#[test]
fn an_unregistered_kind_is_not_implemented() {
    let registry = Registry::new();
    assert!(!registry.is_implemented(ProviderInstanceKind::Hyperion));
    match registry.create(&instance("hyperion"), None) {
        Err(CreateError::NotImplemented { kind, runbook }) => {
            assert_eq!(kind, ProviderInstanceKind::Hyperion);
            assert_eq!(runbook, "RB-10");
        }
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("an instance of a kind nobody registered"),
    }
}

#[test]
fn a_registered_kind_makes_its_instances() {
    let kind = Arc::new(RecordingKind::new(ProviderInstanceKind::Wled, vec![]));
    let mut registry = Registry::new();
    registry.register(kind.clone());
    assert!(registry.is_implemented(ProviderInstanceKind::Wled));
    assert_eq!(
        registry.kinds().collect::<Vec<_>>(),
        [ProviderInstanceKind::Wled]
    );
    let provider = registry.create(&instance("wled"), None).unwrap();
    assert!(provider.element_output().is_some());
    assert!(
        provider.raster_output().is_none(),
        "raster output is reserved"
    );
    assert_eq!(kind.created().len(), 1);
    // Another kind is still absent
    assert!(matches!(
        registry.create(&instance("hue"), None),
        Err(CreateError::NotImplemented { .. })
    ));
}

#[tokio::test]
async fn discovery_gathers_the_registered_kinds_that_were_asked() {
    let mut registry = Registry::new();
    assert!(
        registry
            .discover(&[ProviderInstanceKind::Hyperion])
            .await
            .is_empty()
    );

    registry.register(Arc::new(RecordingKind::new(
        ProviderInstanceKind::Hyperion,
        vec![device("hyperion", "h1")],
    )));
    registry.register(Arc::new(RecordingKind::new(
        ProviderInstanceKind::Wled,
        vec![device("wled", "w1"), device("wled", "w2")],
    )));
    registry.register(Arc::new(
        RecordingKind::new(ProviderInstanceKind::Hue, vec![device("hue", "b1")]).failing(),
    ));

    let ids = |devices: Vec<DiscoveredDevice>| -> Vec<String> {
        devices.iter().map(|d| (*d.id).clone()).collect()
    };
    let all = [
        ProviderInstanceKind::Hyperion,
        ProviderInstanceKind::Wled,
        ProviderInstanceKind::Hue,
    ];
    // A kind that fails adds nothing and doesn't spoil the others
    assert_eq!(ids(registry.discover(&all).await), ["h1", "w1", "w2"]);
    assert_eq!(
        ids(registry.discover(&[ProviderInstanceKind::Wled]).await),
        ["w1", "w2"]
    );
    assert!(registry.discover(&[]).await.is_empty());
}

#[tokio::test]
async fn administration_answers_by_action() {
    let provider = RecordingProvider::new().with_admin("info", json!({"name": "strip"}));
    let read = provider
        .admin("info", AdminMethod::Read, json!(null))
        .await
        .unwrap();
    assert_eq!(read, json!({"name": "strip"}));
    let unknown = provider
        .admin("reboot", AdminMethod::Change, json!(null))
        .await
        .unwrap_err();
    assert_eq!(unknown, AdminError::UnknownAction("reboot".to_owned()));
    assert_eq!((unknown.status(), unknown.code()), (404, "not_found"));
    assert_eq!(provider.events().len(), 2);
    assert!(matches!(
        provider.events()[0],
        Event::Admin {
            method: AdminMethod::Read,
            ..
        }
    ));
}

#[test]
fn admin_errors_carry_their_http_answer() {
    let missing = AdminError::NotImplemented { runbook: "RB-12" };
    assert_eq!((missing.status(), missing.code()), (501, "not_implemented"));
    assert_eq!(missing.to_string(), "arrives with RB-12");
    let device: AdminError = ProviderError::Unreachable("no answer".to_owned()).into();
    assert_eq!((device.status(), device.code()), (502, "provider_error"));
}

#[test]
fn colours_are_truncated_to_bytes_for_the_wire_key() {
    let colors = ElementColors::Rgb(vec![[10.9, 255.0, 300.0], [-4.0, 0.5, 1.99]]);
    assert_eq!(colors.to_bytes(), [10, 255, 255, 0, 0, 1]);
    assert_eq!(colors.len(), 2);
    assert_eq!(colors.color_model(), CapabilityDescriptorColorModel::Rgb);
    let provider = RecordingProvider::new();
    let key = |c: ElementColors| {
        use folo_provider::ElementOutput;
        provider.wire_key(&TargetOutput {
            target: "t".to_owned(),
            colors: c,
        })
    };
    assert_eq!(key(ElementColors::OnOff(vec![true, false])), [1, 0]);
    assert_eq!(
        key(ElementColors::Rgbw(vec![[1.0, 2.0, 3.0, 4.9]])),
        [1, 2, 3, 4]
    );
}
