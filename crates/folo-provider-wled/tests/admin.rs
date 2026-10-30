//! Administration against the fake device: what each read announces, the HTTP body each change
//! sends, and what the device holds afterwards.

use std::sync::Arc;

use folo_provider::{AdminError, AdminMethod, Provider, ProviderError};
use folo_provider_wled::testing::{FakeDevice, MemorySink, segments_of};
use folo_provider_wled::{Config, Transport, WledProvider};
use serde_json::{Value, json};

fn config(pin: Option<&str>) -> Config {
    Config {
        authority: "wled.test".into(),
        host: "wled.test".into(),
        transport: Transport::Ddp,
        timeout_s: 2,
        pin: pin.map(str::to_owned),
        ddp_port: 4048,
        dnrgb_port: None,
    }
}

fn setup(fixture: &str) -> (Arc<FakeDevice>, WledProvider) {
    let device = Arc::new(FakeDevice::fixture(fixture));
    let provider = WledProvider::new(config(None), device.clone(), Arc::new(MemorySink::new()));
    (device, provider)
}

/// What every settings body also carries: the values WLED would reset to defaults if they were
/// left out (the fixtures' `hw.led.rgbwm` 255, `hw.led.fps` 42 and one linked remote).
fn with_preserved(mut body: Value) -> Value {
    body["hw"] = json!({"led": {"rgbwm": 255, "fps": 42}});
    body["nw"] = json!({"linked_remote": ["aabbccddeeff"]});
    body
}

async fn read(provider: &WledProvider, action: &str, body: Value) -> Value {
    provider
        .admin(action, AdminMethod::Read, body)
        .await
        .unwrap()
}

async fn change(provider: &WledProvider, action: &str, body: Value) -> Value {
    provider
        .admin(action, AdminMethod::Change, body)
        .await
        .unwrap()
}

async fn rejected(provider: &WledProvider, action: &str, body: Value) -> String {
    match provider.admin(action, AdminMethod::Change, body).await {
        Err(AdminError::Provider(e)) => e.message().to_owned(),
        other => panic!("expected a provider error, got {other:?}"),
    }
}

#[tokio::test]
async fn info_reports_the_device_its_targets_and_warnings() {
    let (_, provider) = setup("segments");
    let info = read(&provider, "info", Value::Null).await;
    assert_eq!(info["name"], "WLED Desk");
    assert_eq!(info["version"], "16.0.1");
    assert_eq!(info["mac"], "aabbccddeeff");
    assert_eq!(info["leds"]["count"], 90);
    assert_eq!(info["segments"].as_array().unwrap().len(), 3);
    assert_eq!(info["targets"], json!(["strip", "seg:0", "seg:1", "seg:2"]));
    assert_eq!(info["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(info["ui_url"], "http://wled.test/");
}

#[tokio::test]
async fn the_device_ui_is_a_url() {
    let (_, provider) = setup("rgb-strip");
    let answer = read(&provider, "ui", Value::Null).await;
    assert_eq!(answer, json!({"url": "http://wled.test/"}));
}

#[tokio::test]
async fn an_unknown_action_is_unknown() {
    let (_, provider) = setup("rgb-strip");
    let error = provider
        .admin("flash-firmware", AdminMethod::Change, Value::Null)
        .await
        .unwrap_err();
    assert_eq!(error, AdminError::UnknownAction("flash-firmware".into()));
}

#[tokio::test]
async fn power_states_its_change_then_makes_it() {
    let (device, provider) = setup("rgb-strip");

    let announced = read(&provider, "power", json!({"on": false})).await;
    assert_eq!(announced["summary"], "Turn the device off.");
    assert_eq!(
        announced["changes"],
        json!([{"setting": "on", "from": true, "to": false}])
    );
    assert_eq!(
        device.posts("/json/state").len(),
        0,
        "a read changes nothing"
    );

    let done = change(&provider, "power", json!({"on": false})).await;
    assert_eq!(done["changed"], announced["changes"]);
    assert_eq!(done["current"], json!({"on": false}));
    assert_eq!(device.posts("/json/state"), vec![json!({"on": false})]);
    assert_eq!(device.state()["on"], false);
}

#[tokio::test]
async fn a_change_that_changes_nothing_sends_nothing() {
    let (device, provider) = setup("rgb-strip");
    let done = change(&provider, "power", json!({"on": true})).await;
    assert_eq!(done["changed"], json!([]));
    assert_eq!(done["summary"], "The device is already on.");
    assert_eq!(device.posts("/json/state").len(), 0);
}

#[tokio::test]
async fn power_without_a_body_reads_the_state() {
    let (_, provider) = setup("rgb-strip");
    let answer = read(&provider, "power", Value::Null).await;
    assert_eq!(answer["current"], json!({"on": true}));
    assert_eq!(answer["changes"], json!([]));
}

#[tokio::test]
async fn brightness_is_set_and_bounded() {
    let (device, provider) = setup("rgb-strip");
    let announced = read(&provider, "brightness", json!({"bri": 255})).await;
    assert_eq!(
        announced["changes"],
        json!([{"setting": "bri", "from": 128, "to": 255}])
    );
    let done = change(&provider, "brightness", json!({"bri": 255})).await;
    assert_eq!(done["current"], json!({"bri": 255}));
    assert_eq!(device.posts("/json/state"), vec![json!({"bri": 255})]);

    let message = rejected(&provider, "brightness", json!({"bri": 300})).await;
    assert!(message.contains("0–255"), "{message}");
    assert_eq!(device.posts("/json/state").len(), 1);
}

#[tokio::test]
async fn rename_writes_the_server_description() {
    let (device, provider) = setup("rgb-strip");
    let announced = read(&provider, "rename", json!({"name": "Hall"})).await;
    assert!(announced["summary"].as_str().unwrap().contains("\"Hall\""));
    assert_eq!(device.posts("/json/cfg").len(), 0);

    let done = change(&provider, "rename", json!({"name": "Hall"})).await;
    assert_eq!(done["current"], json!({"name": "Hall"}));
    assert_eq!(
        device.posts("/json/cfg"),
        vec![with_preserved(json!({"id": {"name": "Hall"}}))]
    );
    assert_eq!(device.cfg()["id"]["name"], "Hall");
}

#[tokio::test]
async fn rename_refuses_empty_and_long_names() {
    let (device, provider) = setup("rgb-strip");
    assert!(
        rejected(&provider, "rename", json!({"name": "  "}))
            .await
            .contains("1–32")
    );
    let long = "x".repeat(33);
    assert!(
        rejected(&provider, "rename", json!({"name": long}))
            .await
            .contains("1–32")
    );
    assert_eq!(device.posts("/json/cfg").len(), 0);
}

#[tokio::test]
async fn reboot_states_what_it_does_and_sends_rb() {
    let (device, provider) = setup("rgb-strip");
    let announced = read(&provider, "reboot", Value::Null).await;
    assert!(announced["summary"].as_str().unwrap().contains("Restart"));
    assert!(!device.rebooted());

    change(&provider, "reboot", Value::Null).await;
    assert_eq!(device.posts("/json/state"), vec![json!({"rb": true})]);
    assert!(device.rebooted());
}

#[tokio::test]
async fn a_segment_is_created_in_the_lowest_free_id() {
    let (device, provider) = setup("rgb-strip");
    let body = json!({"op": "create", "start": 10, "stop": 20, "name": "Accent", "bri": 200});

    let announced = read(&provider, "segments", body.clone()).await;
    assert_eq!(
        announced["summary"],
        "Create segment 1 over LEDs 10–19 (10 LEDs)."
    );
    assert_eq!(device.posts("/json/state").len(), 0);

    change(&provider, "segments", body).await;
    assert_eq!(
        device.posts("/json/state"),
        vec![json!({"seg": [{"start": 10, "stop": 20, "bri": 200, "n": "Accent", "id": 1}]})]
    );
    assert_eq!(segments_of(&device.state()), [(0, 0, 60), (1, 10, 20)]);
}

#[tokio::test]
async fn a_segment_is_edited_field_by_field() {
    let (device, provider) = setup("segments");
    let announced = read(
        &provider,
        "segments",
        json!({"op": "edit", "id": 2, "start": 65}),
    )
    .await;
    assert!(
        announced["summary"]
            .as_str()
            .unwrap()
            .contains("Edit segment 2")
    );

    change(
        &provider,
        "segments",
        json!({"op": "edit", "id": 2, "start": 65, "name": "R"}),
    )
    .await;
    assert_eq!(
        device.posts("/json/state"),
        vec![json!({"seg": [{"start": 65, "n": "R", "id": 2}]})]
    );
    assert_eq!(segments_of(&device.state())[2], (2, 65, 90));
}

#[tokio::test]
async fn a_segment_is_deleted_by_giving_it_no_leds() {
    let (device, provider) = setup("segments");
    let announced = read(&provider, "segments", json!({"op": "delete", "id": 1})).await;
    assert_eq!(announced["summary"], "Delete segment 1 (LEDs 30–59).");

    change(&provider, "segments", json!({"op": "delete", "id": 1})).await;
    assert_eq!(
        device.posts("/json/state"),
        vec![json!({"seg": [{"id": 1, "stop": 0}]})]
    );
    assert_eq!(segments_of(&device.state()), [(0, 0, 30), (2, 60, 90)]);
}

#[tokio::test]
async fn segment_changes_update_the_targets_the_provider_sends_to() {
    let (_, provider) = setup("rgb-strip");
    change(
        &provider,
        "segments",
        json!({"op": "create", "start": 10, "stop": 20}),
    )
    .await;
    let targets = provider.describe().await.unwrap();
    assert!(
        targets
            .iter()
            .any(|t| t.id == "seg:1" && t.address.start == 10)
    );
}

#[tokio::test]
async fn invalid_segment_requests_are_refused_before_anything_is_sent() {
    let (device, provider) = setup("segments");
    for (body, expected) in [
        (
            json!({"op": "create", "start": 10, "stop": 5}),
            "stop must be above start",
        ),
        (
            json!({"op": "create", "start": 80, "stop": 100}),
            "has 90 LEDs",
        ),
        (
            json!({"op": "create", "id": 0, "start": 0, "stop": 5}),
            "exists",
        ),
        (json!({"op": "create", "start": 0}), "needs start and stop"),
        (json!({"op": "edit", "id": 9, "start": 1}), "no segment 9"),
        (json!({"op": "edit", "start": 1}), "needs a segment id"),
        (json!({"op": "delete", "id": 9}), "no segment 9"),
        (json!({"op": "nonsense"}), "unknown segment operation"),
        (json!({"op": "edit", "id": 0, "stop": 200}), "has 90 LEDs"),
        (json!({"op": "edit", "id": 0, "bri": 999}), "0–255"),
    ] {
        let message = rejected(&provider, "segments", body.clone()).await;
        assert!(message.contains(expected), "{body}: {message}");
    }
    assert_eq!(device.posts("/json/state").len(), 0);
}

#[tokio::test]
async fn realtime_settings_are_checked_and_fixed() {
    let (device, provider) = setup("rgb-strip");

    // The fixture has receive on and gamma off, a 2.5 s timeout and no forced brightness; the
    // instance asks for 2 s
    let announced = read(&provider, "realtime", Value::Null).await;
    assert_eq!(
        announced["changes"],
        json!([
            {"setting": "timeout", "from": 25, "to": 20},
            {"setting": "maxbri", "from": false, "to": true},
        ])
    );
    assert_eq!(device.posts("/json/cfg").len(), 0);

    let done = change(&provider, "realtime", Value::Null).await;
    assert_eq!(done["changed"], announced["changes"]);
    assert_eq!(
        device.posts("/json/cfg"),
        vec![with_preserved(
            json!({"if": {"live": {"timeout": 20, "maxbri": true}}})
        )]
    );
    assert_eq!(device.cfg()["if"]["live"]["timeout"], 20);

    // Now it is right, and a second change sends nothing
    let again = change(&provider, "realtime", Value::Null).await;
    assert_eq!(again["changed"], json!([]));
    assert_eq!(device.posts("/json/cfg").len(), 1);
}

#[tokio::test]
async fn realtime_fix_turns_receive_and_device_gamma_into_what_folo_needs() {
    let (device, provider) = setup("rgb-strip");
    device.edit(|_, _, cfg| {
        cfg["if"]["live"]["en"] = false.into();
        cfg["if"]["live"]["no-gc"] = false.into();
    });
    let done = change(&provider, "realtime", Value::Null).await;
    let settings: Vec<&str> = done["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["setting"].as_str().unwrap())
        .collect();
    assert_eq!(settings, ["en", "timeout", "maxbri", "no-gc"]);
    let live = &device.cfg()["if"]["live"];
    assert_eq!(live["en"], true);
    assert_eq!(live["no-gc"], true);
}

#[tokio::test]
async fn settings_that_need_a_pin_carry_it() {
    let device = Arc::new(FakeDevice::fixture("rgb-strip").with_pin("1234"));
    let provider = WledProvider::new(
        config(Some("1234")),
        device.clone(),
        Arc::new(MemorySink::new()),
    );
    change(&provider, "rename", json!({"name": "Locked"})).await;
    let posts = device.posts("/json/cfg");
    let mut expected = with_preserved(json!({"id": {"name": "Locked"}}));
    expected["pin"] = json!("1234");
    assert_eq!(posts, vec![expected]);
    assert_eq!(device.cfg()["id"]["name"], "Locked");
    assert!(device.cfg().get("pin").is_none());
}

#[tokio::test]
async fn writing_settings_without_the_pin_is_refused() {
    let device = Arc::new(FakeDevice::fixture("rgb-strip").with_pin("1234"));
    let provider = WledProvider::new(config(None), device.clone(), Arc::new(MemorySink::new()));
    // Reading, and everything that doesn't touch settings, still works
    read(&provider, "realtime", Value::Null).await;
    change(&provider, "power", json!({"on": false})).await;
    let error = provider
        .admin("rename", AdminMethod::Change, json!({"name": "X"}))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdminError::Provider(ProviderError::Refused(_))
    ));
    assert_ne!(device.cfg()["id"]["name"], "X");
}

#[tokio::test]
async fn a_settings_write_keeps_what_wled_would_reset() {
    let (device, provider) = setup("rgbw-strip");
    device.edit(|_, _, cfg| cfg["hw"]["led"]["rgbwm"] = 0.into());
    change(&provider, "rename", json!({"name": "Kept"})).await;
    let cfg = device.cfg();
    assert_eq!(cfg["hw"]["led"]["rgbwm"], 0, "the global auto-white mode");
    assert_eq!(cfg["hw"]["led"]["fps"], 42);
    assert_eq!(cfg["nw"]["linked_remote"], json!(["aabbccddeeff"]));
}

#[tokio::test]
async fn a_new_segment_cannot_name_an_id_beyond_the_next_one() {
    let (device, provider) = setup("rgb-strip");
    let message = rejected(
        &provider,
        "segments",
        json!({"op": "create", "id": 7, "start": 1, "stop": 5}),
    )
    .await;
    assert!(message.contains("next free"), "{message}");
    assert_eq!(device.posts("/json/state").len(), 0);
    change(
        &provider,
        "segments",
        json!({"op": "create", "id": 1, "start": 1, "stop": 5}),
    )
    .await;
    assert_eq!(segments_of(&device.state()).len(), 2);
}

#[tokio::test]
async fn names_are_limited_in_bytes() {
    let (_, provider) = setup("rgb-strip");
    // 17 two-byte characters are 34 bytes
    let message = rejected(&provider, "rename", json!({"name": "é".repeat(17)})).await;
    assert!(message.contains("32 bytes"), "{message}");
}

#[tokio::test]
async fn a_reboot_does_not_read_the_device_back() {
    let (device, provider) = setup("rgb-strip");
    let done = change(&provider, "reboot", Value::Null).await;
    assert_eq!(done["current"], Value::Null);
    assert_eq!(device.requests().last().unwrap().method, "POST");
}

struct DiesAfterWrite(Arc<FakeDevice>, std::sync::atomic::AtomicBool);

impl folo_provider_wled::DeviceApi for DiesAfterWrite {
    fn get<'a>(
        &'a self,
        path: &'a str,
    ) -> folo_provider::BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            if self.1.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ProviderError::Unreachable("gone".into()));
            }
            self.0.get(path).await
        })
    }

    fn post<'a>(
        &'a self,
        path: &'a str,
        body: &'a Value,
    ) -> folo_provider::BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            let answer = self.0.post(path, body).await;
            self.1.store(true, std::sync::atomic::Ordering::SeqCst);
            answer
        })
    }

    fn authority(&self) -> String {
        "dies".into()
    }
}

#[tokio::test]
async fn a_failed_read_back_after_a_change_is_a_warning_not_an_error() {
    let device = Arc::new(FakeDevice::fixture("rgb-strip"));
    let api = Arc::new(DiesAfterWrite(device.clone(), false.into()));
    let provider = WledProvider::new(config(None), api, Arc::new(MemorySink::new()));
    let done = change(&provider, "power", json!({"on": false})).await;
    assert!(done["warning"].as_str().unwrap().contains("read back"));
    assert_eq!(device.state()["on"], false);
}

#[tokio::test]
async fn an_unreachable_device_is_a_provider_error() {
    struct Down;
    impl folo_provider_wled::DeviceApi for Down {
        fn get<'a>(
            &'a self,
            _: &'a str,
        ) -> folo_provider::BoxFuture<'a, Result<Value, ProviderError>> {
            Box::pin(async { Err(ProviderError::Unreachable("down".into())) })
        }
        fn post<'a>(
            &'a self,
            _: &'a str,
            _: &'a Value,
        ) -> folo_provider::BoxFuture<'a, Result<Value, ProviderError>> {
            Box::pin(async { Err(ProviderError::Unreachable("down".into())) })
        }
        fn authority(&self) -> String {
            "down".into()
        }
    }
    let provider = WledProvider::new(config(None), Arc::new(Down), Arc::new(MemorySink::new()));
    let error = provider
        .admin("info", AdminMethod::Read, Value::Null)
        .await
        .unwrap_err();
    assert_eq!(error.status(), 502);
}
