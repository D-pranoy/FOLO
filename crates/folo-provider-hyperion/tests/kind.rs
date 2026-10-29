//! The kind the node registers: instances from `providers.json` become providers, and the
//! documents' settings and secrets are what they use.

mod common;

use folo_protocol::generated::{ProviderInstance, ProviderInstanceKind, ProviderSecrets};
use folo_provider::{AdminMethod, CreateError, ProviderError, ProviderKind, Registry};
use folo_provider_hyperion::testing::FakeHyperion;
use folo_provider_hyperion::{HyperionConfig, HyperionKind, KEEPALIVE};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn instance(settings: &serde_json::Value) -> ProviderInstance {
    serde_json::from_value(json!({
        "id": "hyperion",
        "kind": "hyperion",
        "name": "Hyperion",
        "settings": settings,
    }))
    .expect("an instance")
}

#[test]
fn settings_become_the_config() {
    let instance = instance(&json!({
        "kind": "hyperion", "host": "10.0.0.5", "port": 19445, "priority": 150, "origin": "My origin"
    }));
    let secrets: ProviderSecrets =
        serde_json::from_value(json!({"token": "c1b2a3d4-0000-4000-8000-123456789abc"})).unwrap();
    let config = HyperionConfig::from_instance(&instance, Some(&secrets)).unwrap();
    assert_eq!(
        config,
        HyperionConfig {
            host: "10.0.0.5".to_owned(),
            port: 19445,
            priority: 150,
            origin: "My origin".to_owned(),
            token: Some("c1b2a3d4-0000-4000-8000-123456789abc".to_owned()),
        }
    );
}

#[test]
fn the_defaults_are_the_documented_ones() {
    let config =
        HyperionConfig::from_instance(&instance(&json!({"kind": "hyperion"})), None).unwrap();
    assert_eq!(
        (
            config.host.as_str(),
            config.port,
            config.priority,
            config.origin.as_str()
        ),
        ("localhost", 19444, 100, "FOLO")
    );
    assert_eq!(config.token, None);
}

#[test]
fn an_origin_over_twenty_characters_is_not_used() {
    let instance = instance(&json!({"kind": "hyperion", "origin": "ok"}));
    let mut long = instance.clone();
    if let folo_protocol::generated::ProviderSettings::HyperionSettings(s) = &mut long.settings {
        s.origin = "x".repeat(20).parse().unwrap();
    }
    assert!(HyperionConfig::from_instance(&long, None).is_ok());
    assert!(
        "x".repeat(21)
            .parse::<folo_protocol::generated::HyperionSettingsOrigin>()
            .is_err()
    );
}

#[test]
fn an_origin_must_have_four_to_twenty_characters() {
    // Hyperion's color command takes 4 to 20, and the documents only cap the top
    for (origin, ok) in [
        ("abc", false),
        ("abcd", true),
        ("a".repeat(20).as_str(), true),
    ] {
        let mut instance = instance(&json!({"kind": "hyperion"}));
        if let folo_protocol::generated::ProviderSettings::HyperionSettings(s) =
            &mut instance.settings
        {
            s.origin = origin.parse().unwrap();
        }
        assert_eq!(
            HyperionConfig::from_instance(&instance, None).is_ok(),
            ok,
            "{origin}"
        );
    }
}

#[test]
fn settings_of_another_kind_are_refused() {
    let mut other = instance(&json!({"kind": "hyperion"}));
    other.settings = serde_json::from_value(json!({
        "kind": "wled", "host": "wled.local", "transport": "ddp", "timeout_s": 2
    }))
    .unwrap();
    let error = HyperionConfig::from_instance(&other, None).unwrap_err();
    assert!(matches!(error, ProviderError::Failed(_)));
}

#[tokio::test]
async fn the_registry_makes_hyperion_providers_and_discovery_finds_nothing() {
    let mut registry = Registry::new();
    registry.register(Arc::new(HyperionKind));
    assert!(registry.is_implemented(ProviderInstanceKind::Hyperion));
    assert!(
        registry
            .discover(&[ProviderInstanceKind::Hyperion])
            .await
            .is_empty()
    );

    let provider = registry
        .create(&instance(&json!({"kind": "hyperion"})), None)
        .map_err(|e: CreateError| e.to_string())
        .expect("a provider");
    assert_eq!(provider.delivery_policy().keepalive, KEEPALIVE);
    assert_eq!(KEEPALIVE, Duration::from_millis(500));
    assert!(provider.element_output().is_some());
    assert!(provider.raster_output().is_none());
}

#[tokio::test]
async fn a_provider_made_by_the_kind_drives_the_fake() {
    let fake = FakeHyperion::start().await;
    let instance = instance(&json!({"kind": "hyperion", "host": fake.host(), "port": fake.port()}));
    let provider = HyperionKind.create(&instance, None).unwrap();
    provider.connect().await.unwrap();
    assert_eq!(provider.describe().await.unwrap().len(), 1);
    let ui = provider
        .admin("ui", AdminMethod::Read, serde_json::Value::Null)
        .await
        .unwrap();
    assert!(ui["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));
    provider.disconnect().await;
}
