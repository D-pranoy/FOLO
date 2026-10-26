//! The types' JSON against the fixtures in specs/fixtures and the types
//! generated from specs/schema, so the hand-written shapes can't drift.

use folo_capture_core::error::CaptureError;
use folo_capture_core::types::{
    Bars, BlackBarMode, BlackBarPreset, CaptureRequest, CaptureState, CaptureStatus, Profile,
    Screen, ScreensListResult,
};
use folo_protocol::generated as generated_types;
use serde_json::{Value, json};

fn fixture_params(name: &str) -> Value {
    let path = format!(
        "{}/../../specs/fixtures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let message: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    message["params"].clone()
}

/// Equal as JSON: `60` and `60.0` are the same number.
fn same_json(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_json(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| same_json(x, y)))
        }
        _ => left == right,
    }
}

fn request() -> CaptureRequest {
    serde_json::from_value(fixture_params("capture.start")).unwrap()
}

#[test]
fn capture_start_params_read_and_write_as_the_fixture() {
    let params = fixture_params("capture.start");
    let request: CaptureRequest = serde_json::from_value(params.clone()).unwrap();
    assert_eq!(request.screen, "\\\\?\\DISPLAY#TST0001#5&0&UID1");
    assert_eq!((request.width, request.height), (128, 72));
    assert_eq!(request.profile, Profile::Video);
    assert_eq!(request.black_bars.preset, BlackBarPreset::Auto);
    assert_eq!(request.black_bars.mode, BlackBarMode::Default);
    assert!(same_json(&serde_json::to_value(&request).unwrap(), &params));
    request.validate().unwrap();
    serde_json::from_value::<generated_types::CaptureStartParams>(params)
        .expect("the generated type takes it");
}

#[test]
fn the_forced_presets_and_the_restore_token_keep_their_names() {
    let mut params = fixture_params("capture.start");
    params["black_bars"] = json!({"preset": "21:9", "mode": "osd", "threshold": 0.5});
    params["profile"] = json!("game");
    params["restore_token"] = json!("abc");
    let request: CaptureRequest = serde_json::from_value(params.clone()).unwrap();
    assert_eq!(request.black_bars.preset, BlackBarPreset::Cinema);
    assert_eq!(request.restore_token.as_deref(), Some("abc"));
    assert!(same_json(&serde_json::to_value(&request).unwrap(), &params));
    serde_json::from_value::<generated_types::CaptureStartParams>(params).unwrap();
}

#[test]
fn a_request_with_a_field_nobody_defined_is_refused() {
    let mut params = fixture_params("capture.start");
    params["extra"] = json!(1);
    assert!(serde_json::from_value::<CaptureRequest>(params).is_err());
}

#[test]
fn every_limit_of_a_request_is_checked() {
    let broken = |change: fn(&mut CaptureRequest)| {
        let mut request = request();
        change(&mut request);
        matches!(request.validate(), Err(CaptureError::InvalidRequest(_)))
    };
    assert!(broken(|r| r.screen = "x".repeat(129)));
    assert!(broken(|r| r.width = 0));
    assert!(broken(|r| r.width = 481));
    assert!(broken(|r| r.height = 0));
    assert!(broken(|r| r.height = 271));
    assert!(broken(|r| r.rate_hz = 0.5));
    assert!(broken(|r| r.rate_hz = 60.5));
    assert!(broken(|r| r.rate_hz = f64::NAN));
    assert!(broken(|r| r.black_bars.threshold = 0.009));
    assert!(broken(|r| r.black_bars.threshold = 0.51));
    let mut edge = request();
    (
        edge.width,
        edge.height,
        edge.rate_hz,
        edge.black_bars.threshold,
    ) = (480, 270, 60.0, 0.5);
    edge.validate().unwrap();
    (
        edge.width,
        edge.height,
        edge.rate_hz,
        edge.black_bars.threshold,
    ) = (1, 1, 1.0, 0.01);
    edge.validate().unwrap();
}

#[test]
fn the_status_fixture_reads_and_writes_the_same() {
    let params = fixture_params("notify.capture.status");
    let status: CaptureStatus = serde_json::from_value(params.clone()).unwrap();
    assert_eq!(status.state, CaptureState::Capturing);
    assert_eq!(status.backend, "dda");
    assert_eq!(status.bars.top, 0.125);
    assert!(same_json(&serde_json::to_value(&status).unwrap(), &params));
    serde_json::from_value::<generated_types::CaptureStatusParams>(params).unwrap();
}

#[test]
fn every_state_and_the_off_status_are_in_the_generated_schema() {
    for (state, name) in [
        (CaptureState::Off, "off"),
        (CaptureState::Starting, "starting"),
        (CaptureState::WaitingConsent, "waiting_consent"),
        (CaptureState::Capturing, "capturing"),
        (CaptureState::Paused, "paused"),
        (CaptureState::Error, "error"),
    ] {
        let mut status = CaptureStatus::off();
        status.state = state;
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(value["state"], name);
        serde_json::from_value::<generated_types::CaptureStatusParams>(value).unwrap();
    }
}

#[test]
fn a_status_with_all_its_optional_fields_is_accepted_by_the_generated_type() {
    let mut status = CaptureStatus::off();
    status.state = CaptureState::Error;
    status.restore_token = Some("token".to_owned());
    status.message = Some("denied".to_owned());
    status.settings_uri = Some("ms-settings:privacy-graphicscaptureprogrammatic".to_owned());
    let value = serde_json::to_value(&status).unwrap();
    serde_json::from_value::<generated_types::CaptureStatusParams>(value).unwrap();
}

#[test]
fn screens_read_and_write_as_the_schema_has_them() {
    let list = ScreensListResult {
        screens: vec![Screen {
            id: "portal".to_owned(),
            name: "Screen".to_owned(),
            width: 0,
            height: 0,
            primary: true,
        }],
    };
    let value = serde_json::to_value(&list).unwrap();
    assert_eq!(
        value,
        json!({"screens": [{"id": "portal", "name": "Screen", "width": 0, "height": 0, "primary": true}]})
    );
    serde_json::from_value::<generated_types::ScreensListResult>(value).unwrap();
}

#[test]
fn bars_fit_the_picture_only_when_a_row_and_a_column_are_left() {
    let bars = Bars {
        top: 2,
        bottom: 3,
        left: 4,
        right: 5,
    };
    assert!(bars.fit(10, 6));
    assert!(!bars.fit(9, 6));
    assert!(!bars.fit(10, 5));
    assert!(Bars::default().fit(1, 1));
}

#[test]
fn bars_become_shares_of_the_frame() {
    let fractions = Bars {
        top: 9,
        bottom: 9,
        left: 0,
        right: 12,
    }
    .fractions(48, 72);
    assert_eq!(
        (
            fractions.top,
            fractions.bottom,
            fractions.left,
            fractions.right
        ),
        (0.125, 0.125, 0.0, 0.25)
    );
    assert_eq!(
        Bars {
            top: 1,
            bottom: 0,
            left: 0,
            right: 0
        }
        .fractions(0, 0)
        .top,
        0.0
    );
}
