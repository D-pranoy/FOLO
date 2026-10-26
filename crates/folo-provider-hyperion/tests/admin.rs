//! Basic administration (RB-10 step 4, ADR-0014): every action's read and change, what a read
//! says a change would do, and the API token.

mod common;

use folo_provider::{AdminError, AdminMethod, Provider, ProviderError};
use folo_provider_hyperion::testing::{FakeHyperion, edge_leds};
use folo_provider_hyperion::{HyperionConfig, HyperionProvider};
use serde_json::{Value, json};

use common::{provider, provider_with_token};

/// As long as Hyperion's own app tokens (a UUID with dashes).
const TOKEN: &str = "c1b2a3d4-0000-4000-8000-123456789abc";
const WRONG_TOKEN: &str = "ffffffff-0000-4000-8000-123456789abc";

async fn connected(fake: &FakeHyperion) -> std::sync::Arc<HyperionProvider> {
    let provider = provider(fake);
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();
    provider
}

async fn get(provider: &HyperionProvider, action: &str, query: Value) -> Value {
    provider
        .admin(action, AdminMethod::Read, query)
        .await
        .unwrap_or_else(|e| panic!("reading {action}: {e}"))
}

async fn post(provider: &HyperionProvider, action: &str, body: Value) -> Value {
    provider
        .admin(action, AdminMethod::Change, body)
        .await
        .unwrap_or_else(|e| panic!("changing {action}: {e}"))
}

/// A request Hyperion refused, or one the provider refused to send.
async fn refused(provider: &HyperionProvider, action: &str, body: Value) -> String {
    match provider.admin(action, AdminMethod::Change, body).await {
        Err(AdminError::Provider(ProviderError::Refused(why)) | AdminError::BadRequest(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A body that isn't what the action takes is 400 `bad_body`, not a provider error.
#[tokio::test]
async fn a_malformed_body_is_a_bad_request() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    for (action, body) in [
        ("priorities", json!({})),
        ("source", json!({"auto": false})),
        ("token", json!({"token": 5})),
    ] {
        let error = provider
            .admin(action, AdminMethod::Change, body.clone())
            .await
            .unwrap_err();
        assert!(
            matches!(error, AdminError::BadRequest(_)),
            "{action} {body}: {error:?}"
        );
        assert_eq!((error.status(), error.code()), (400, "bad_body"));
    }
}

#[tokio::test]
async fn an_unknown_action_is_unknown() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    let error = provider
        .admin("device-config", AdminMethod::Read, Value::Null)
        .await
        .unwrap_err();
    assert_eq!(error, AdminError::UnknownAction("device-config".to_owned()));
    assert_eq!(error.status(), 404);
}

#[tokio::test]
async fn an_unconnected_provider_answers_unreachable() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let error = provider
        .admin("priorities", AdminMethod::Read, Value::Null)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdminError::Provider(ProviderError::Unreachable(_))
    ));
    assert_eq!(error.status(), 502);
}

#[tokio::test]
async fn instances_are_listed_and_one_is_selected() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(4));
    fake.set_instance(2, "Stopped", false, edge_leds(4));
    fake.add_priority(1, 60, "Desk app", "JSON API");
    let provider = connected(&fake).await;

    let read = get(&provider, "instances", Value::Null).await;
    assert_eq!(read["selected"], 0);
    assert_eq!(read["instances"].as_array().unwrap().len(), 3);
    assert_eq!(
        read["instances"][1],
        json!({"instance": 1, "name": "Desk", "running": true})
    );
    assert_eq!(read["instances"][2]["running"], false);
    assert!(
        read["change"]
            .as_str()
            .unwrap()
            .contains("nothing changes on Hyperion")
    );

    let preview = get(&provider, "instances", json!({"instance": "1"})).await;
    assert!(preview["change"].as_str().unwrap().contains("instance 1"));
    assert_eq!(preview["selected"], 0, "a read changes nothing");

    let changed = post(&provider, "instances", json!({"instance": 1})).await;
    assert_eq!(changed["selected"], 1);
    assert!(changed.get("change").is_none());
    // The other actions now apply to it
    let priorities = get(&provider, "priorities", Value::Null).await;
    assert_eq!(priorities["priorities"][0]["priority"], 60);

    assert!(
        refused(&provider, "instances", json!({"instance": 2}))
            .await
            .contains("isn't running")
    );
    assert!(
        refused(&provider, "instances", json!({}))
            .await
            .contains("missing")
    );
}

#[tokio::test]
async fn priorities_are_listed_and_cleared() {
    let fake = FakeHyperion::start().await;
    fake.add_priority(0, 50, "Other app", "JSON API");
    fake.add_priority(0, 100, "FOLO", "JSON API");
    let provider = connected(&fake).await;

    let read = get(&provider, "priorities", Value::Null).await;
    let list = read["priorities"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["priority"], 50);
    assert_eq!(list[0]["origin"], "Other app");
    assert_eq!(list[0]["component"], "COLOR");
    assert_eq!(list[0]["visible"], true);
    assert_eq!(list[1]["visible"], false);
    assert_eq!(read["auto"], true);

    let preview = get(&provider, "priorities", json!({"priority": "50"})).await;
    let said = preview["change"].as_str().unwrap();
    assert!(
        said.contains("priority 50") && said.contains("Other app"),
        "{said}"
    );
    assert_eq!(fake.priorities(0).len(), 2, "a read clears nothing");

    let changed = post(&provider, "priorities", json!({"priority": 50})).await;
    assert_eq!(changed["priorities"].as_array().unwrap().len(), 1);
    assert_eq!(fake.priorities(0), vec![(100, "FOLO".to_owned())]);

    post(&provider, "priorities", json!({"priority": -1})).await;
    assert_eq!(fake.priorities(0).len(), 0);
}

#[tokio::test]
async fn a_priority_out_of_range_is_not_sent() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    for bad in [
        json!({"priority": 0}),
        json!({"priority": 254}),
        json!({"priority": "x"}),
    ] {
        refused(&provider, "priorities", bad).await;
    }
    refused(&provider, "priorities", json!({})).await;
    assert!(fake.commands("clear").is_empty());
}

#[tokio::test]
async fn components_are_listed_and_switched() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;

    let read = get(&provider, "components", Value::Null).await;
    let smoothing = read["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "SMOOTHING")
        .unwrap();
    assert_eq!(smoothing["enabled"], true);

    let preview = get(
        &provider,
        "components",
        json!({"component": "smoothing", "enabled": "false"}),
    )
    .await;
    let said = preview["change"].as_str().unwrap();
    assert!(
        said.contains("SMOOTHING") && said.contains("from on to off"),
        "{said}"
    );
    assert_eq!(fake.component(0, "SMOOTHING"), Some(true));

    post(
        &provider,
        "components",
        json!({"component": "SMOOTHING", "enabled": false}),
    )
    .await;
    assert_eq!(fake.component(0, "SMOOTHING"), Some(false));
    for name in ["LEDDEVICE", "BLACKBORDER", "AUDIO", "V4L"] {
        post(
            &provider,
            "components",
            json!({"component": name, "enabled": false}),
        )
        .await;
        assert_eq!(fake.component(0, name), Some(false), "{name}");
    }
    let changed = post(
        &provider,
        "components",
        json!({"component": "AUDIO", "enabled": true}),
    )
    .await;
    assert_eq!(fake.component(0, "AUDIO"), Some(true));
    assert!(
        changed["components"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "AUDIO" && c["enabled"] == true)
    );

    // Only the five of the administration; the others stay in Hyperion
    refused(
        &provider,
        "components",
        json!({"component": "FORWARDER", "enabled": false}),
    )
    .await;
    refused(&provider, "components", json!({"component": "AUDIO"})).await;
    assert_eq!(fake.component(0, "FORWARDER"), Some(true));
}

#[tokio::test]
async fn the_source_is_automatic_or_pinned() {
    let fake = FakeHyperion::start().await;
    fake.add_priority(0, 50, "Other app", "JSON API");
    fake.add_priority(0, 100, "FOLO", "JSON API");
    let provider = connected(&fake).await;

    let read = get(&provider, "source", Value::Null).await;
    assert_eq!(read["auto"], true);
    assert_eq!(read["visible"], 50);

    let pinned = post(&provider, "source", json!({"priority": 100})).await;
    assert_eq!(pinned["auto"], false);
    assert_eq!(pinned["visible"], 100);
    assert_eq!(fake.source(0), Some((false, Some(100))));

    let preview = get(&provider, "source", json!({"auto": "true"})).await;
    assert!(
        preview["change"]
            .as_str()
            .unwrap()
            .contains("automatically")
    );
    assert_eq!(
        fake.source(0),
        Some((false, Some(100))),
        "a read changes nothing"
    );

    let auto = post(&provider, "source", json!({"auto": true})).await;
    assert_eq!(auto["auto"], true);
    assert_eq!(fake.source(0), Some((true, None)));
    let sourceselect = fake.commands("sourceselect");
    assert_eq!(sourceselect.len(), 2);
    assert_eq!(sourceselect[1].command["auto"], true);

    refused(&provider, "source", json!({"auto": true, "priority": 50})).await;
    refused(&provider, "source", json!({"auto": false})).await;
    refused(&provider, "source", json!({"priority": 77})).await;
}

#[tokio::test]
async fn brightness_is_read_and_set() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    assert_eq!(
        get(&provider, "brightness", Value::Null).await["brightness"],
        100
    );

    let preview = get(&provider, "brightness", json!({"brightness": 40})).await;
    let said = preview["change"].as_str().unwrap();
    assert!(said.contains("from 100% to 40%"), "{said}");
    assert_eq!(fake.brightness(0), Some(100));

    let changed = post(&provider, "brightness", json!({"brightness": 40})).await;
    assert_eq!(changed["brightness"], 40);
    assert_eq!(fake.brightness(0), Some(40));
    let adjustment = fake.commands("adjustment");
    assert_eq!(
        adjustment[0].command["adjustment"],
        json!({"brightness": 40, "id": "A"})
    );

    post(&provider, "brightness", json!({"brightness": 0})).await;
    assert_eq!(fake.brightness(0), Some(0));
    refused(&provider, "brightness", json!({"brightness": 101})).await;
    refused(&provider, "brightness", json!({"brightness": -1})).await;
    refused(&provider, "brightness", json!({})).await;
    assert_eq!(fake.brightness(0), Some(0));
}

#[tokio::test]
async fn a_request_names_its_instance() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(4));
    let provider = connected(&fake).await;
    post(
        &provider,
        "brightness",
        json!({"brightness": 30, "instance": 1}),
    )
    .await;
    assert_eq!(fake.brightness(1), Some(30));
    assert_eq!(fake.brightness(0), Some(100));
    refused(
        &provider,
        "brightness",
        json!({"brightness": 30, "instance": 5}),
    )
    .await;
}

#[tokio::test]
async fn hyperions_own_ui_is_linked() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    let read = get(&provider, "ui", Value::Null).await;
    assert_eq!(read["url"], "http://127.0.0.1:8090/");
}

#[tokio::test]
async fn without_the_token_a_protected_hyperion_refuses() {
    let fake = FakeHyperion::start().await;
    fake.require_token(TOKEN);
    let provider = provider(&fake);
    let error = provider.connect().await.unwrap_err();
    assert!(matches!(error, ProviderError::Refused(_)), "{error:?}");
    assert!(error.message().contains("No Authorization"), "{error:?}");
}

#[tokio::test]
async fn the_configured_token_logs_in_first() {
    let fake = FakeHyperion::start().await;
    fake.require_token(TOKEN);
    let provider = provider_with_token(&fake, TOKEN);
    provider.connect().await.unwrap();
    let targets = provider.describe().await.unwrap();
    assert_eq!(targets.len(), 1);
    let logins = fake.commands("authorize");
    assert_eq!(logins[0].command["subcommand"], "login");
    assert_eq!(logins[0].command["token"], TOKEN);
    // The login is the first thing sent on the connection
    assert_eq!(fake.received()[0].name(), "authorize");

    let read = get(&provider, "token", Value::Null).await;
    assert_eq!(read["required"], true);
    assert_eq!(read["configured"], true);
    assert_eq!(read["persisted"], false);
    assert!(
        !read.to_string().contains(TOKEN),
        "the token is never echoed"
    );
}

#[tokio::test]
async fn a_wrong_token_is_refused() {
    let fake = FakeHyperion::start().await;
    fake.require_token(TOKEN);
    let provider = provider_with_token(&fake, WRONG_TOKEN);
    let error = provider.connect().await.unwrap_err();
    assert!(matches!(error, ProviderError::Refused(_)), "{error:?}");
}

#[tokio::test]
async fn a_token_is_tried_and_then_used() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    fake.require_token(TOKEN);
    // Hyperion started protecting itself after the connection was made; this one stays authorised
    let read = get(&provider, "token", Value::Null).await;
    assert_eq!(read["configured"], false);
    assert_eq!(read["required"], true);

    let bad = refused(&provider, "token", json!({"token": WRONG_TOKEN})).await;
    assert!(bad.contains("authorize"), "{bad}");
    // Hyperion never answers a token under 36 characters, so none is sent
    let short = refused(&provider, "token", json!({"token": "nope"})).await;
    assert!(short.contains("at least 36"), "{short}");
    assert_eq!(
        fake.commands("authorize").len(),
        2,
        "tokenRequired and the one login"
    );
    assert_eq!(
        get(&provider, "token", Value::Null).await["configured"],
        false
    );

    let preview = get(&provider, "token", json!({"token": TOKEN})).await;
    assert!(preview["change"].as_str().unwrap().contains("secrets.json"));
    assert_eq!(preview["configured"], false, "a read adopts nothing");

    let accepted = post(&provider, "token", json!({"token": TOKEN})).await;
    assert_eq!(accepted["configured"], true);
    assert!(!accepted.to_string().contains(TOKEN));
    // A reconnect now logs in with it
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();

    refused(&provider, "token", json!({})).await;
    refused(&provider, "token", json!({"token": ""})).await;
}

#[tokio::test]
async fn a_configured_token_that_is_too_short_is_refused_without_asking_hyperion() {
    let fake = FakeHyperion::start().await;
    fake.require_token(TOKEN);
    let provider = provider_with_token(&fake, "short");
    let error = provider.connect().await.unwrap_err();
    assert!(matches!(error, ProviderError::Refused(_)), "{error:?}");
    assert!(fake.commands("authorize").is_empty());
}

#[test]
fn the_config_never_prints_its_token() {
    let config = HyperionConfig {
        host: "h".to_owned(),
        port: 1,
        priority: 100,
        origin: "FOLO".to_owned(),
        token: Some(TOKEN.to_owned()),
    };
    let printed = format!("{config:?}");
    assert!(!printed.contains(TOKEN), "{printed}");
    assert!(printed.contains("<redacted>"));
}

#[tokio::test]
async fn a_read_ignores_a_token() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    fake.require_token(TOKEN);
    let read = get(&provider, "token", json!({"token": TOKEN})).await;
    assert_eq!(read["configured"], false);
    assert!(!read.to_string().contains(TOKEN));
    assert!(
        fake.commands("authorize")
            .iter()
            .all(|c| c.command["subcommand"] != "login")
    );
}

#[tokio::test(start_paused = true)]
async fn a_silent_hyperion_ends_a_request_instead_of_hanging() {
    let fake = FakeHyperion::start().await;
    let provider = connected(&fake).await;
    fake.set_silent(true);
    let error = provider
        .admin("priorities", AdminMethod::Read, Value::Null)
        .await
        .unwrap_err();
    let AdminError::Provider(ProviderError::Unreachable(why)) = error else {
        panic!("expected unreachable");
    };
    assert!(why.contains("in time"), "{why}");
}

#[tokio::test]
async fn administration_reopens_a_connection_that_died() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(4));
    let provider = connected(&fake).await;
    fake.drop_connections();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let changed = post(
        &provider,
        "brightness",
        json!({"brightness": 20, "instance": 1}),
    )
    .await;
    assert_eq!(changed["brightness"], 20);
    assert_eq!(fake.brightness(1), Some(20));
}

#[tokio::test]
async fn an_instance_selected_before_the_first_describe_is_described() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(4));
    let provider = provider(&fake);
    provider.connect().await.unwrap();
    post(&provider, "instances", json!({"instance": 1})).await;
    let targets = provider.describe().await.unwrap();
    assert_eq!(targets.len(), 2);
}

#[tokio::test]
async fn a_stopped_and_restarted_instance_comes_back() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(4));
    let provider = connected(&fake).await;
    fake.set_running(1, false);
    assert_eq!(provider.describe().await.unwrap().len(), 1);
    fake.set_running(1, true);
    let targets = provider.describe().await.unwrap();
    assert_eq!(
        targets.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["0", "1"]
    );
}
