//! The parity path (RB-08 step 7 and "Done when", `docs/architecture/pipeline.md#parity-mode`):
//! the engine's frames go through the compositor, projection with the migration's 1:1 manual
//! mapping, identity adapters and a provider, and come out as the same bytes.
//!
//! - every reactive frame of `tests/golden/renderer/*.png`, compared exactly
//! - some of them through a real sender, to a recording provider
//! - every migrated configuration in `tests/golden/migration/*/out/`
//! - the ambient light: `tests/golden/ambient/` inputs against the **existing**
//!   `renderer/ambient-*.png` frames, within ±1 per channel
//!
//! These tests only read the goldens.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::{STEP_US, core_of, golden, read_png, strip_frame, strip_zone, zones_of};
use folo_controller::{Media, OutputCore};
use folo_protocol::generated::ProvidersDocument;
use folo_provider::testing::RecordingProvider;
use folo_provider::{DeliveryPolicy, SenderConfig, spawn_sender};
use serde::Deserialize;
use serde_json::json;

const INSTANCE: &str = "rec";

/// The reactive renderer goldens: everything but the ambient light's.
fn reactive_goldens() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(golden().join("renderer"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .filter(|p| {
            !p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ambient-")
        })
        .collect();
    paths.sort();
    paths
}

/// Feeds every row of a golden to a strip zone and checks what the provider gets and what the
/// `target:` topic shows.
fn check_golden(path: &std::path::Path) {
    let image = read_png(path);
    let leds = u32::try_from(image.width).unwrap();
    let mut core = core_of(&[strip_zone("z", leds, INSTANCE, &json!({}))]);
    let mut now = 1_000_000;
    for (i, row) in image.rows.iter().enumerate() {
        now += STEP_US;
        core.activity("local", None, true, now);
        core.frame("main", "local", &strip_frame(i as u32, row), now)
            .unwrap();
        let out = core.step(now);
        let frame = &out.frames[INSTANCE];
        assert_eq!(frame.targets.len(), 1);
        assert_eq!(
            frame.targets[0].colors.to_bytes(),
            *row,
            "{} frame {i}: the provider's bytes",
            path.display()
        );
        assert_eq!(out.targets["z-strip"], *row, "{} frame {i}", path.display());
    }
}

#[test]
fn identity_adapters_reproduce_every_reactive_golden_frame_exactly() {
    let goldens = reactive_goldens();
    assert!(goldens.len() > 200, "{} goldens", goldens.len());
    for path in &goldens {
        check_golden(path);
    }
}

/// A sender on Tokio's paused clock: each frame is submitted and a sender tick passes, so every
/// frame that changed reaches the provider.
#[tokio::test(start_paused = true)]
async fn a_recording_provider_gets_the_golden_frames_through_its_sender() {
    for name in [
        "builtin-fx-bass-burst-144.png",
        "builtin-genre-ambient-61.png",
        "mode-fire-hits.png",
    ] {
        let path = golden().join("renderer").join(name);
        let image = read_png(&path);
        let leds = u32::try_from(image.width).unwrap();
        let zone = strip_zone("z", leds, INSTANCE, &json!({}));
        let mut core = core_of(std::slice::from_ref(&zone));
        let provider = Arc::new(RecordingProvider::new());
        let instance = serde_json::from_value(json!({
            "id": INSTANCE, "kind": "hyperion", "rate_hz": 40,
        }))
        .unwrap();
        let config =
            SenderConfig::from_instance(&instance, DeliveryPolicy::default(), &zone.targets);
        let sender = spawn_sender(provider.clone(), config).unwrap();
        let mut now = 1_000_000;
        let mut wanted: Vec<Vec<u8>> = Vec::new();
        for (i, row) in image.rows.iter().enumerate() {
            now += 25_000;
            core.activity("local", None, true, now);
            core.frame("main", "local", &strip_frame(i as u32, row), now)
                .unwrap();
            let mut out = core.step(now);
            sender.submit(out.frames.remove(INSTANCE).unwrap());
            if wanted.last() != Some(row) {
                wanted.push(row.clone());
            }
            // one tick at 40 Hz
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        sender.shutdown().await;
        let mut sent: Vec<Vec<u8>> = Vec::new();
        for (_, frame) in provider.sends() {
            let bytes = frame.targets[0].colors.to_bytes();
            // a keepalive repeats the last frame
            if sent.last() != Some(&bytes) {
                sent.push(bytes);
            }
        }
        assert_eq!(sent, wanted, "{name}");
    }
}

/// Every migrated configuration drives its provider: each target gets its surface's frame,
/// byte for byte, on the instance the migration bound it to.
#[test]
fn migrated_configurations_drive_their_providers() {
    for case in fs::read_dir(golden().join("migration")).unwrap() {
        let out_dir = case.unwrap().path().join("out");
        let zones = zones_of(&out_dir.join("zones.json"));
        let providers: ProvidersDocument =
            serde_json::from_str(&fs::read_to_string(out_dir.join("providers.json")).unwrap())
                .unwrap();
        let mut core = core_of(&zones);
        let mut now = 1_000_000;
        for step in 0..30u32 {
            now += STEP_US;
            core.activity("local", None, true, now);
            let mut sent = BTreeMap::new();
            for zone in &zones {
                for surface in &zone.surfaces {
                    let cells = surface.resolution.width.get() as usize;
                    let bytes: Vec<u8> = (0..cells * 3)
                        .map(|i| ((i as u32 * 7 + step * 13) % 256) as u8)
                        .collect();
                    core.frame(&surface.id, "local", &strip_frame(step, &bytes), now)
                        .unwrap();
                    sent.insert(surface.id.to_string(), bytes);
                }
            }
            let out = core.step(now);
            for zone in &zones {
                for target in &zone.targets {
                    let instance = target.provider.instance.as_str();
                    assert!(
                        providers.providers.iter().any(|p| *p.id == instance),
                        "{} binds {instance}",
                        out_dir.display()
                    );
                    let mapping = zone
                        .mappings
                        .iter()
                        .find(|m| *m.target == *target.id)
                        .unwrap();
                    let frame = &out.frames[instance];
                    let got = frame
                        .targets
                        .iter()
                        .find(|t| *t.target == *target.id)
                        .unwrap();
                    assert_eq!(
                        got.colors.to_bytes(),
                        sent[mapping.surface.as_str()],
                        "{} step {step}",
                        out_dir.display()
                    );
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct Manifest {
    cases: BTreeMap<String, Case>,
}

#[derive(Deserialize)]
struct Colors {
    color: String,
    style: String,
    level: f64,
    idle_after: f64,
}

#[derive(Deserialize)]
struct Case {
    reactive: String,
    ambient: Colors,
    on_pause: bool,
    master: f64,
    paused: bool,
    input: String,
    expected: String,
    dts: Vec<f64>,
    active: String,
}

/// The largest difference per channel between the core's frames and the legacy renderer's.
fn ambient_case(case: &Case) -> i16 {
    let input = read_png(&golden().join("ambient").join(&case.input));
    let expected = read_png(&golden().join(&case.expected));
    let leds = u32::try_from(input.width).unwrap();
    let extra = json!({
        "reactive": case.reactive,
        "ambient": {
            "color": case.ambient.color, "style": case.ambient.style,
            "level": case.ambient.level, "idle_after": case.ambient.idle_after,
            "on_pause": case.on_pause,
        },
    });
    let mut zone = strip_zone("z", leds, INSTANCE, &extra);
    zone.surfaces[0].master = case.master;
    let mut core: OutputCore = core_of(&[zone]);
    core.media(
        "local",
        if case.paused {
            Media::Paused
        } else {
            Media::Playing
        },
    );
    // a first step starts the clock, so the first frame moves the light by its own step, as the
    // legacy renderer's first call does
    let mut now = 1_000_000_u64;
    core.step(now);
    let mut biggest = 0;
    for (i, (row, want)) in input.rows.iter().zip(&expected.rows).enumerate() {
        now += (case.dts[i] * 1e6).round() as u64;
        core.activity("local", None, case.active.as_bytes()[i] == b'1', now);
        core.frame("main", "local", &strip_frame(i as u32, row), now)
            .unwrap();
        let out = core.step(now);
        let got = out.frames[INSTANCE].targets[0].colors.to_bytes();
        for (g, w) in got.iter().zip(want) {
            biggest = biggest.max((i16::from(*g) - i16::from(*w)).abs());
        }
        assert_eq!(out.targets["z-strip"], got, "frame {i}");
    }
    biggest
}

/// The ambient light through the whole path, on the controller's clock in whole microseconds.
/// Every case stays within ±1 per channel but `ambient-auto-paused`, which is within 4: there
/// the legacy float sum of the steps reaches the 0.3 s pause exactly, the clock's sum of rounded
/// microseconds lands a step later, and the fade starts one step late. The compositor's own test
/// (`crates/folo-compositor/tests/ambient_golden.rs`) holds every case to ±1 in the legacy
/// path's own seconds and shows the same one-step shift on a clock.
#[test]
fn the_ambient_light_matches_the_legacy_frames() {
    let manifest: Manifest =
        serde_json::from_str(&fs::read_to_string(golden().join("ambient/manifest.json")).unwrap())
            .unwrap();
    let mut checked = 0;
    for (name, case) in &manifest.cases {
        let biggest = ambient_case(case);
        let bound = if name == "ambient-auto-paused" { 4 } else { 1 };
        assert!(biggest <= bound, "{name}: {biggest} per channel");
        checked += usize::from(case.expected.starts_with("renderer/"));
    }
    assert_eq!(checked, 5, "the existing renderer goldens");
}
