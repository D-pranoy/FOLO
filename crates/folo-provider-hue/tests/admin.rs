//! Basic administration: every change says what it would change first, and does only that.

mod support;

use std::sync::Arc;

use folo_provider::{AdminError, AdminMethod, Provider, ProviderError};
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::fake_bridge::{AREA, BRIDGE_ID, FakeBridge};

async fn connected(bridge: &FakeBridge) -> Arc<dyn Provider> {
    let provider = support::provider(bridge);
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();
    provider
}

async fn read(
    provider: &Arc<dyn Provider>,
    action: &str,
    body: Value,
) -> Result<Value, AdminError> {
    provider.admin(action, AdminMethod::Read, body).await
}

async fn change(
    provider: &Arc<dyn Provider>,
    action: &str,
    body: Value,
) -> Result<Value, AdminError> {
    provider.admin(action, AdminMethod::Change, body).await
}

fn writes(bridge: &FakeBridge) -> usize {
    bridge
        .requests()
        .iter()
        .filter(|r| !r.starts_with("GET"))
        .count()
}

#[tokio::test]
async fn bridge_information_and_update_status() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let info = read(&provider, "bridge", Value::Null).await.unwrap();
    assert_eq!(info["model_id"], "BSB002");
    assert_eq!(info["api_version"], "1.65.0");
    assert!(
        info["bridge_id"]
            .as_str()
            .unwrap()
            .eq_ignore_ascii_case(BRIDGE_ID)
    );

    bridge.state().update_state = "allreadytoinstall".into();
    let status = read(&provider, "update_status", Value::Null).await.unwrap();
    assert_eq!(status["update_state"], "allreadytoinstall");
    assert_eq!(status["auto_install"], true);
    assert_eq!(status["software_version"], "1976154040");
}

#[tokio::test]
async fn lights_are_listed_renamed_and_identified_with_a_preview_first() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let lights = read(&provider, "lights", Value::Null).await.unwrap();
    let strip = lights["lights"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == "light-strip")
        .unwrap();
    assert_eq!(strip["gradient"], true);
    assert_eq!(strip["entertainment"], true);

    let before = writes(&bridge);
    let preview = read(
        &provider,
        "light_rename",
        json!({"id": "light-lamp", "name": "Reading lamp"}),
    )
    .await
    .unwrap();
    assert!(
        preview["would"]
            .as_str()
            .unwrap()
            .contains("Renames the light \"Desk lamp\" to \"Reading lamp\"")
    );
    assert_eq!(writes(&bridge), before, "a read changes nothing");

    let done = change(
        &provider,
        "light_rename",
        json!({"id": "light-lamp", "name": "Reading lamp"}),
    )
    .await
    .unwrap();
    assert_eq!(done["name"], "Reading lamp");
    let described = provider.describe().await.unwrap();
    assert!(
        described.iter().any(|t| t.name == "Reading lamp"),
        "the new name is described"
    );

    let preview = read(&provider, "light_identify", json!({"id": "light-strip"}))
        .await
        .unwrap();
    assert!(preview["would"].as_str().unwrap().contains("blink"));
    assert_eq!(bridge.state().identified, Vec::<String>::new());
    change(&provider, "light_identify", json!({"id": "light-strip"}))
        .await
        .unwrap();
    assert_eq!(bridge.state().identified, vec!["light-strip".to_owned()]);
}

#[tokio::test]
async fn bad_requests_are_refused_without_touching_the_bridge() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let before = writes(&bridge);
    for (action, body) in [
        ("light_rename", json!({"id": "light-lamp"})),
        ("light_rename", json!({"id": "light-lamp", "name": ""})),
        (
            "light_rename",
            json!({"id": "light-lamp", "name": "x".repeat(33)}),
        ),
        ("light_rename", json!({"id": "nope", "name": "x"})),
        ("light_identify", json!({})),
        ("entertainment_create", json!({"name": "A", "lights": []})),
        (
            "entertainment_create",
            json!({"name": "A", "configuration_type": "bogus", "lights": [{"light": "light-lamp"}]}),
        ),
        (
            "entertainment_create",
            json!({"name": "A", "lights": [{"light": "light-lamp", "positions": [{"x": 2.0}]}]}),
        ),
        (
            "entertainment_create",
            json!({"name": "A", "lights": [{"light": "ghost"}]}),
        ),
        ("entertainment_update", json!({"id": AREA})),
        ("entertainment_delete", json!({"id": "nope"})),
    ] {
        let error = change(&provider, action, body.clone()).await.unwrap_err();
        assert!(
            matches!(&error, AdminError::Provider(ProviderError::Failed(m)) if m.starts_with("invalid request")),
            "{action} {body}: {error}"
        );
    }
    assert_eq!(writes(&bridge), before);
}

#[tokio::test]
async fn rooms_are_read_only() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let rooms = read(&provider, "rooms", Value::Null).await.unwrap();
    let list = rooms["rooms"].as_array().unwrap();
    let room = list.iter().find(|r| r["kind"] == "room").unwrap();
    assert_eq!(room["name"], "Living room");
    assert_eq!(room["lights"], json!(["Desk lamp"]));
    let zone = list.iter().find(|r| r["kind"] == "zone").unwrap();
    assert_eq!(zone["lights"], json!(["Gradient strip"]));
    for action in [
        "rooms",
        "bridge",
        "lights",
        "entertainment",
        "update_status",
    ] {
        assert!(
            matches!(
                change(&provider, action, Value::Null).await,
                Err(AdminError::UnknownAction(_))
            ),
            "{action} is read only"
        );
    }
}

#[tokio::test]
async fn entertainment_areas_are_created_updated_and_deleted() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    let request = json!({
        "name": "Music", "configuration_type": "music",
        "lights": [{"light": "light-lamp", "positions": [{"x": -0.5, "y": 0.5, "z": 0.0}]}, {"service": "svc-strip"}],
    });

    let before = writes(&bridge);
    let preview = read(&provider, "entertainment_create", request.clone())
        .await
        .unwrap();
    assert!(preview["would"].as_str().unwrap().contains("2 light(s)"));
    assert_eq!(writes(&bridge), before);

    let created = change(&provider, "entertainment_create", request)
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    let sent = bridge
        .state()
        .requests
        .iter()
        .rev()
        .find(|r| r.0 == "POST")
        .unwrap()
        .2
        .clone();
    assert_eq!(sent["metadata"]["name"], "Music");
    assert_eq!(sent["configuration_type"], "music");
    let locations = sent["locations"]["service_locations"].as_array().unwrap();
    assert_eq!(
        locations[0]["service"],
        json!({"rid": "svc-lamp", "rtype": "entertainment"})
    );
    assert_eq!(locations[0]["positions"][0]["x"], -0.5);
    assert_eq!(
        locations[1]["positions"],
        json!([{"x": 0.0, "y": 0.0, "z": 0.0}])
    );

    // The new area's lights are targets now
    let described = provider.describe().await.unwrap();
    assert!(
        described
            .iter()
            .any(|t| t.id == format!("{id}/svc-strip") && t.address.channels == vec![1, 2, 3])
    );
    let areas = read(&provider, "entertainment", Value::Null).await.unwrap();
    assert_eq!(areas["areas"].as_array().unwrap().len(), 2);

    let update = json!({"id": id, "name": "Party"});
    let preview = read(&provider, "entertainment_update", update.clone())
        .await
        .unwrap();
    assert!(
        preview["would"]
            .as_str()
            .unwrap()
            .contains("renames it to \"Party\"")
    );
    assert_eq!(bridge.area(&id)["metadata"]["name"], "Music");
    change(&provider, "entertainment_update", update)
        .await
        .unwrap();
    assert_eq!(bridge.area(&id)["metadata"]["name"], "Party");

    let preview = read(&provider, "entertainment_delete", json!({"id": id}))
        .await
        .unwrap();
    assert!(preview["would"].as_str().unwrap().contains("Deletes"));
    assert!(
        bridge
            .state()
            .configurations
            .iter()
            .any(|c| c["id"] == id.as_str())
    );
    change(&provider, "entertainment_delete", json!({"id": id}))
        .await
        .unwrap();
    assert!(
        !bridge
            .state()
            .configurations
            .iter()
            .any(|c| c["id"] == id.as_str())
    );
}

#[tokio::test]
async fn an_area_that_folo_streams_to_is_not_edited() {
    let bridge = FakeBridge::start().await;
    let provider = connected(&bridge).await;
    provider
        .element_output()
        .unwrap()
        .start("11111111-2222-3333-4444-555555555555/svc-lamp")
        .await
        .unwrap();
    for (action, body) in [
        ("entertainment_update", json!({"id": AREA, "name": "Other"})),
        ("entertainment_delete", json!({"id": AREA})),
    ] {
        let error = change(&provider, action, body).await.unwrap_err();
        assert!(error.to_string().contains("streaming"), "{error}");
    }
    assert_eq!(bridge.area(AREA)["metadata"]["name"], "TV");
}

#[tokio::test]
async fn an_unknown_action_is_not_found_and_the_instance_must_be_connected() {
    let bridge = FakeBridge::start().await;
    let provider = support::provider(&bridge);
    assert!(matches!(
        read(&provider, "bridge", Value::Null).await,
        Err(AdminError::Provider(ProviderError::Unreachable(_)))
    ));
    provider.connect().await.unwrap();
    let error = read(&provider, "scenes", Value::Null).await.unwrap_err();
    assert_eq!(error.status(), 404);
    assert_eq!(error.code(), "not_found");
}

#[tokio::test]
async fn the_event_stream_delivers_the_bridges_events() {
    let bridge = FakeBridge::start().await;
    let trust = folo_provider_hue::trust::BridgeTrust {
        bridge_id: BRIDGE_ID.into(),
        anchors: folo_provider_hue::trust::Anchors::Custom(vec![bridge.authority_der.clone()]),
        pinned: None,
    };
    let client = folo_provider_hue::bridge::client_for(
        &bridge.endpoint,
        trust,
        Some(support::fake_bridge::USERNAME.into()),
        folo_provider_hue::rest::RestLimits::default(),
    )
    .unwrap();
    let mut events = Box::pin(client.events().await.unwrap());
    // The stream is open once the bridge has the subscriber; give the request a moment to land
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    bridge.send_event(&json!([{"type": "update", "data": [{"id": AREA, "type": "entertainment_configuration", "status": "active"}]}]));
    let batch = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(batch[0]["type"], "update");
    assert_eq!(batch[0]["data"][0]["status"], "active");
}
