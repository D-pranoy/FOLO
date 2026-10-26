//! What a device offers as targets, from each fixture, and what is said about it.

use std::sync::Arc;

use folo_protocol::generated::{
    CapabilityDescriptorAddressing as Addressing, CapabilityDescriptorColorModel as Model,
    CapabilityDescriptorDelivery, TargetKind,
};
use folo_provider::Provider;
use folo_provider_wled::testing::{FIXTURES, FakeDevice, MemorySink, fixture};
use folo_provider_wled::{Config, Description, Device, Transport, WledProvider};

fn device_of(name: &str) -> Device {
    let documents = fixture(name);
    Device::from_json(
        &documents["info"],
        &documents["state"],
        Some(&documents["cfg"]),
    )
}

fn describe(name: &str) -> Description {
    Description::of(&device_of(name))
}

/// `(id, kind, first LED, LEDs, colour model, sends white)` of every target.
fn summary(description: &Description) -> Vec<(String, TargetKind, u32, u32, Model, bool)> {
    description
        .targets
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                t.kind,
                t.first_pixel,
                t.count,
                t.color_model,
                t.send_white,
            )
        })
        .collect()
}

fn row(
    id: &str,
    kind: TargetKind,
    first: u32,
    count: u32,
    model: Model,
    white: bool,
) -> (String, TargetKind, u32, u32, Model, bool) {
    (id.to_owned(), kind, first, count, model, white)
}

fn config() -> Config {
    Config {
        authority: "wled.test".into(),
        host: "wled.test".into(),
        transport: Transport::Ddp,
        timeout_s: 2,
        pin: None,
        ddp_port: 4048,
        dnrgb_port: None,
    }
}

#[test]
fn every_fixture_describes_something() {
    for name in FIXTURES {
        assert!(!describe(name).targets.is_empty(), "{name}");
    }
}

#[test]
fn an_rgb_strip_is_a_line_and_its_segment() {
    let description = describe("rgb-strip");
    assert_eq!(
        summary(&description),
        vec![
            row("strip", TargetKind::Line, 0, 60, Model::Rgb, false),
            row("seg:0", TargetKind::Line, 0, 60, Model::Rgb, false),
        ]
    );
    assert_eq!(description.targets[0].name, "WLED Kitchen");
    assert_eq!(description.warnings.len(), 0);
    assert_eq!(description.base_pixel, 0);
}

#[test]
fn segments_are_sub_targets_and_grouping_or_spacing_is_warned_about() {
    let description = describe("segments");
    assert_eq!(
        summary(&description),
        vec![
            row("strip", TargetKind::Line, 0, 90, Model::Rgb, false),
            row("seg:0", TargetKind::Line, 0, 30, Model::Rgb, false),
            row("seg:1", TargetKind::Line, 30, 30, Model::Rgb, false),
            row("seg:2", TargetKind::Line, 60, 30, Model::Rgb, false),
        ]
    );
    let names: Vec<&str> = description
        .targets
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names[1..], ["Left", "Middle", "Right"]);
    // Segment 1 groups 2 and leaves 1; the others are plain (grp 1, spc 0)
    assert_eq!(description.warnings.len(), 1, "{:?}", description.warnings);
    assert!(description.warnings[0].starts_with("Segment 1 groups 2"));
}

#[test]
fn an_rgbw_strip_sends_white_when_the_bus_honours_it() {
    let description = describe("rgbw-strip");
    assert_eq!(
        summary(&description),
        vec![
            row("strip", TargetKind::Line, 0, 30, Model::Rgbw, true),
            row("seg:0", TargetKind::Line, 0, 30, Model::Rgbw, true),
        ]
    );
}

#[test]
fn an_auto_white_mode_that_derives_white_hides_the_white_channel() {
    // Accurate mode computes the white itself, so our white would be ignored
    let description = describe("rgbw-auto-white");
    assert_eq!(description.targets[0].color_model, Model::Rgb);
    assert!(!description.targets[0].send_white);
}

#[test]
fn a_global_auto_white_mode_overrides_the_buses() {
    let documents = fixture("rgbw-strip");
    let mut cfg = documents["cfg"].clone();
    cfg["hw"]["led"]["rgbwm"] = 1.into(); // Brighter, for every bus
    let device = Device::from_json(&documents["info"], &documents["state"], Some(&cfg));
    assert_eq!(Description::of(&device).targets[0].color_model, Model::Rgb);
}

#[test]
fn a_cct_strip_is_cct() {
    let description = describe("cct-strip");
    assert_eq!(description.targets[0].color_model, Model::Cct);
    assert_eq!(description.targets[0].count, 10);
}

#[test]
fn pwm_and_relay_outputs_are_point_targets_in_their_bus_models() {
    let description = describe("analog-and-relay");
    assert_eq!(
        summary(&description),
        vec![
            row("strip", TargetKind::Line, 0, 30, Model::Rgb, false),
            row("seg:0", TargetKind::Line, 0, 30, Model::Rgb, false),
            row("bus:1", TargetKind::Point, 30, 1, Model::Rgbcct, true),
            row("bus:2", TargetKind::Point, 31, 1, Model::Onoff, false),
            row("bus:3", TargetKind::Point, 32, 1, Model::Dimmer, true),
        ]
    );
    let described = description.described();
    assert_eq!(described[2].capabilities.addressing, Addressing::Single);
    assert_eq!(described[2].elements, 1);
}

#[test]
fn a_device_of_outputs_only_has_points_and_no_strip() {
    assert_eq!(
        summary(&describe("analog-rgb")),
        vec![row("bus:0", TargetKind::Point, 0, 1, Model::Rgb, false)]
    );
    assert_eq!(
        summary(&describe("on-off")),
        vec![row("bus:0", TargetKind::Point, 0, 1, Model::Onoff, false)]
    );
}

#[test]
fn a_matrix_is_a_grid() {
    let description = describe("matrix-2d");
    assert_eq!(
        summary(&description),
        vec![row("matrix", TargetKind::Grid, 0, 128, Model::Rgb, false)]
    );
    assert_eq!(description.targets[0].grid, Some((16, 8)));
    let described = description.described();
    assert_eq!(described[0].layouts.len(), 1, "the grid offers its shape");
}

#[test]
fn main_segment_only_offers_the_main_segment_with_relative_indices() {
    let description = describe("main-segment-only");
    assert_eq!(
        summary(&description),
        vec![row("seg:1", TargetKind::Line, 20, 40, Model::Rgb, false)]
    );
    assert_eq!(description.base_pixel, 20);
    assert!(description.warnings[0].contains("main segment only"));
}

#[test]
fn described_targets_carry_the_address_and_the_streaming_capabilities() {
    let described = describe("segments").described();
    let middle = &described[2];
    assert_eq!(middle.id, "seg:1");
    assert_eq!(middle.kind, TargetKind::Line);
    assert_eq!(middle.address.start, 30);
    assert_eq!(middle.address.count.get(), 30);
    assert_eq!(middle.elements, 30);
    assert_eq!(middle.capabilities.addressing, Addressing::Pixel);
    assert_eq!(
        middle.capabilities.delivery,
        CapabilityDescriptorDelivery::Streaming
    );
    assert!((middle.capabilities.limits.max_rate_hz - 66.0).abs() < 1e-9);
}

#[test]
fn a_non_default_dmx_start_address_is_warned_about() {
    let documents = fixture("rgb-strip");
    let mut cfg = documents["cfg"].clone();
    cfg["if"]["live"]["dmx"]["addr"] = 7.into();
    let device = Device::from_json(&documents["info"], &documents["state"], Some(&cfg));
    let warnings = Description::of(&device).warnings;
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("DMX start address is 7"));
}

#[test]
fn a_device_whose_settings_are_locked_still_offers_its_strip() {
    let documents = fixture("rgb-strip");
    let device = Device::from_json(&documents["info"], &documents["state"], None);
    let description = Description::of(&device);
    assert_eq!(description.targets[0].id, "strip");
    assert_eq!(description.targets[0].count, 60);
    assert!(description.warnings[0].contains("PIN"));
}

#[tokio::test]
async fn the_provider_describes_through_the_api() {
    let device = Arc::new(FakeDevice::fixture("segments"));
    let provider = WledProvider::new(config(), device.clone(), Arc::new(MemorySink::new()));
    let targets = provider.describe().await.unwrap();
    let ids: Vec<&str> = targets.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["strip", "seg:0", "seg:1", "seg:2"]);
    let paths: Vec<String> = device.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, ["/json/info", "/json/state", "/json/cfg"]);
}

#[tokio::test]
async fn a_pin_unlocks_the_settings_for_describe() {
    let locked = Arc::new(FakeDevice::fixture("rgbw-strip").with_pin("1234"));

    // Without the PIN: the strip from info alone, as RGB, and a warning
    let provider = WledProvider::new(config(), locked.clone(), Arc::new(MemorySink::new()));
    let targets = provider.describe().await.unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].capabilities.color_model, Model::Rgb);

    // With it: the settings are read and the white channel is known to be honoured
    let mut with_pin = config();
    with_pin.pin = Some("1234".into());
    let provider = WledProvider::new(with_pin, locked.clone(), Arc::new(MemorySink::new()));
    let targets = provider.describe().await.unwrap();
    assert_eq!(targets[0].capabilities.color_model, Model::Rgbw);
    assert!(
        locked
            .posts("/json/state")
            .iter()
            .any(|b| b["pin"] == "1234")
    );
}

#[tokio::test]
async fn a_wrong_pin_is_refused() {
    let locked = Arc::new(FakeDevice::fixture("rgb-strip").with_pin("1234"));
    let mut wrong = config();
    wrong.pin = Some("0000".into());
    let provider = WledProvider::new(wrong, locked, Arc::new(MemorySink::new()));
    let error = provider.describe().await.unwrap_err();
    assert!(matches!(error, folo_provider::ProviderError::Refused(_)));
}
