//! The bytes of `color` and `clear` against what the legacy sender writes
//! (`tests/golden/hyperion/legacy.json`, made from the legacy client by
//! `scripts/make_golden.py --only hyperion`), on the wire from the fake Hyperion.

mod common;

use std::time::Duration;

use folo_provider::Provider;
use folo_provider_hyperion::testing::FakeHyperion;
use folo_provider_hyperion::{HyperionConfig, HyperionProvider, wire};
use serde_json::Value;

use common::{frame, pass, start_sender};

const GOLDEN: &str = include_str!("../../../tests/golden/hyperion/legacy.json");

struct Case {
    name: String,
    priority: u16,
    origin: String,
    levels: Vec<u8>,
    color: String,
    clear: String,
}

fn cases() -> Vec<Case> {
    let golden: Value = serde_json::from_str(GOLDEN).expect("the golden is JSON");
    golden["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().unwrap().to_owned(),
            priority: u16::try_from(c["priority"].as_u64().unwrap()).unwrap(),
            origin: c["origin"].as_str().unwrap().to_owned(),
            levels: c["levels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
                .collect(),
            color: c["color"].as_str().unwrap().to_owned(),
            clear: c["clear"].as_str().unwrap().to_owned(),
        })
        .collect()
}

#[test]
fn the_golden_has_its_cases() {
    assert!(cases().len() >= 7);
}

#[test]
fn color_lines_equal_the_legacy_bytes() {
    for case in cases() {
        let line = wire::color_line(&case.levels, case.priority, &case.origin);
        assert_eq!(
            String::from_utf8(line).unwrap(),
            case.color,
            "color: {}",
            case.name
        );
    }
}

#[test]
fn clear_lines_equal_the_legacy_bytes() {
    for case in cases() {
        let line = wire::clear_line(case.priority);
        assert_eq!(
            String::from_utf8(line).unwrap(),
            case.clear,
            "clear: {}",
            case.name
        );
    }
}

/// The provider's frames on a socket: levels truncated toward zero, one line each, and the line
/// the legacy client wrote for the same bytes.
#[tokio::test]
async fn the_provider_writes_the_legacy_bytes_on_the_socket() {
    let fake = FakeHyperion::start().await;
    for case in cases() {
        let provider = HyperionProvider::new(HyperionConfig {
            host: fake.host(),
            port: fake.port(),
            priority: case.priority,
            origin: case.origin.clone(),
            token: None,
        });
        provider.connect().await.expect("connects");
        provider.describe().await.expect("describes");
        let before = fake.color_frames().len();
        // Fractions the engine's 0-255 scale carries, which the provider drops
        let levels: Vec<[f32; 3]> = case
            .levels
            .chunks(3)
            .map(|c| {
                [
                    f32::from(c[0]) + 0.75,
                    f32::from(c[1]) + 0.5,
                    f32::from(c[2]) + 0.99,
                ]
            })
            .collect();
        let output = provider.element_output().expect("an element output");
        output.start("0").await.expect("starts");
        let frame = frame("0", &levels);
        output.send(&frame).await.expect("sends");
        pass(Duration::from_millis(50)).await;
        let sent = fake.color_frames();
        let last = sent.last().expect("a frame");
        assert_eq!(sent.len(), before + 1, "{}", case.name);
        assert_eq!(format!("{}\n", last.line), case.color, "{}", case.name);
        output.release("0").await.expect("releases");
        pass(Duration::from_millis(50)).await;
        let clears = fake.commands("clear");
        let clear = clears.last().expect("a clear");
        assert_eq!(format!("{}\n", clear.line), case.clear, "{}", case.name);
        provider.disconnect().await;
    }
}

/// Nothing goes out for an empty frame, as in the legacy sender.
#[tokio::test(start_paused = true)]
async fn an_empty_frame_sends_nothing() {
    let fake = FakeHyperion::start().await;
    let provider = common::provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    sender.submit(frame("0", &[]));
    pass(Duration::from_secs(1)).await;
    assert!(fake.color_frames().is_empty());
    sender.shutdown().await;
}
