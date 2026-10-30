//! Describe from `serverinfo` (RB-10 step 3): line targets, LED rectangles on the TV plane, one
//! target and one connection per running instance, layout watching, and the proportional
//! scaling of a changed LED count.

mod common;

use std::num::NonZeroU64;
use std::time::Duration;

use folo_protocol::generated::{
    Address, CapabilityDescriptorAddressing, CapabilityDescriptorColorModel,
    CapabilityDescriptorDelivery, Geometry, ManualRange, Plane, Span, TargetKind, Vec3,
};
use folo_provider::{Provider, ProviderState, spawn_sender};
use folo_provider_hyperion::layout::{
    LayoutChange, default_tv_plane, layout_changes, place_on_plane, scale_address, scale_ranges,
    scale_span,
};
use folo_provider_hyperion::serverinfo::LedRect;
use folo_provider_hyperion::testing::{FakeHyperion, edge_leds};

use common::{frame, output, pass, provider, sender_config};

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} is not {b}");
}

fn positions(target: &folo_provider::DescribedTarget) -> Vec<Vec3> {
    let Geometry::LineGeometry(line) = &target.layouts[0].geometry else {
        panic!("a line layout");
    };
    assert_eq!(line.kind, "line");
    line.positions.clone()
}

#[tokio::test]
async fn a_line_target_is_described_from_serverinfo() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    provider.connect().await.unwrap();
    let targets = provider.describe().await.unwrap();
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target.id, "0");
    assert_eq!(target.name, "FOLO test");
    assert_eq!(target.kind, TargetKind::Line);
    assert_eq!(target.elements, 10);
    assert_eq!(target.address.start, 0);
    assert_eq!(target.address.count.get(), 10);
    let caps = &target.capabilities;
    assert_eq!(caps.color_model, CapabilityDescriptorColorModel::Rgb);
    assert_eq!(caps.delivery, CapabilityDescriptorDelivery::Streaming);
    assert_eq!(caps.addressing, CapabilityDescriptorAddressing::Pixel);
    assert_eq!(target.layouts.len(), 1);
    assert_eq!(positions(target).len(), 10);
    // describe is a read: it sent nothing but the setup commands
    assert!(fake.color_frames().is_empty());
}

#[tokio::test]
async fn led_rectangles_are_placed_on_the_default_tv_plane() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    provider.connect().await.unwrap();
    let target = provider.describe().await.unwrap().remove(0);
    let at = positions(&target);
    // Ten LEDs along the bottom edge: x from the horizontal centre, z from the vertical one, on a
    // 1.2 x 0.68 m TV centred on the origin, in the screen's plane (y = 0)
    near(at[0].x, -0.6 + 1.2 * 0.05);
    near(at[9].x, -0.6 + 1.2 * 0.95);
    near(at[0].z, 0.34 - 0.68 * 0.95);
    for position in &at {
        near(position.y, 0.0);
    }
}

#[test]
fn a_turned_tv_turns_the_layout_with_it() {
    let leds = [LedRect {
        hmin: 0.0,
        hmax: 0.5,
        vmin: 0.0,
        vmax: 0.5,
    }];
    // A TV turned a quarter about +z: its width runs along +y, and its screen faces +x
    let plane = Plane {
        origin: Vec3 {
            x: 1.0,
            y: -0.6,
            z: 0.34,
        },
        u: Vec3 {
            x: 0.0,
            y: 1.2,
            z: 0.0,
        },
        v: Vec3 {
            x: 0.0,
            y: 0.0,
            z: -0.68,
        },
    };
    let at = place_on_plane(&leds, &plane);
    near(at[0].x, 1.0);
    near(at[0].y, -0.6 + 1.2 * 0.25);
    near(at[0].z, 0.34 - 0.68 * 0.25);
    let default = place_on_plane(&leds, &default_tv_plane());
    near(default[0].x, -0.6 + 1.2 * 0.25);
}

#[tokio::test]
async fn every_running_instance_is_a_target_with_a_connection_of_its_own() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(4));
    fake.set_instance(2, "Stopped", false, edge_leds(8));
    let provider = provider(&fake);
    provider.connect().await.unwrap();
    let targets = provider.describe().await.unwrap();
    let ids: Vec<_> = targets
        .iter()
        .map(|t| (t.id.as_str(), t.elements))
        .collect();
    assert_eq!(ids, [("0", 10), ("1", 4)]);
    assert_eq!(fake.connections(), 2);
}

#[tokio::test(start_paused = true)]
async fn frames_reach_the_instance_they_name() {
    let fake = FakeHyperion::start().await;
    fake.set_instance(1, "Desk", true, edge_leds(2));
    let provider = provider(&fake);
    let mut config = sender_config(&provider, &["0", "1"]);
    config.describe_every = None;
    let sender = spawn_sender(provider.clone(), config).unwrap();
    sender.submit(folo_provider::ElementFrame::new(
        0,
        vec![
            output("0", &[[1.0, 1.0, 1.0]]),
            output("1", &[[2.0, 2.0, 2.0]]),
        ],
    ));
    pass(Duration::from_millis(300)).await;
    let sent = fake.color_frames();
    let on = |instance| {
        sent.iter()
            .filter(|f| f.instance == instance)
            .map(folo_provider_hyperion::testing::Received::levels)
            .next()
    };
    assert_eq!(on(0), Some(vec![1, 1, 1]));
    assert_eq!(on(1), Some(vec![2, 2, 2]));
    let switches = fake.commands("instance");
    assert!(switches.iter().any(|s| s.command["instance"] == 1));
    sender.shutdown().await;
    pass(Duration::from_millis(100)).await;
    let clears: Vec<_> = fake.commands("clear").iter().map(|c| c.instance).collect();
    assert_eq!(clears.len(), 2);
    assert!(clears.contains(&0) && clears.contains(&1));
}

#[tokio::test(start_paused = true)]
async fn a_changed_led_count_is_found_by_watching() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let mut config = sender_config(&provider, &["0"]);
    config.describe_every = Some(folo_provider_hyperion::DESCRIBE_EVERY);
    let sender = spawn_sender(provider.clone(), config).unwrap();
    let mut described = sender.described();
    pass(Duration::from_millis(200)).await;
    let before = described.borrow_and_update().clone().expect("described");
    assert_eq!(before[0].elements, 10);

    fake.set_leds(0, edge_leds(20));
    pass(Duration::from_secs(6)).await;
    let after = described
        .borrow_and_update()
        .clone()
        .expect("described again");
    assert_eq!(after[0].elements, 20);
    assert_eq!(
        layout_changes(&before, &after),
        vec![LayoutChange {
            target: "0".to_owned(),
            old_count: 10,
            new_count: 20,
        }]
    );
    assert_eq!(layout_changes(&after, &after).len(), 0);
    assert_eq!(sender.status().borrow().state, ProviderState::Connected);
    // Output carries on while it watches
    sender.submit(frame("0", &[[3.0, 3.0, 3.0]; 20]));
    pass(Duration::from_millis(200)).await;
    assert_eq!(fake.color_frames().last().unwrap().levels().len(), 60);
    sender.shutdown().await;
}

#[test]
fn spans_keep_their_share_of_the_strip_as_the_studio_scales_them() {
    // The studio: start' = round(start * r), end' = max(start', round((end + 1) * r) - 1),
    // with Python's rounding (halves go to the even number)
    assert_eq!(scale_span(0, 10, 10, 20), (0, 20));
    assert_eq!(scale_span(0, 50, 150, 300), (0, 100));
    assert_eq!(scale_span(50, 50, 150, 300), (100, 100));
    assert_eq!(scale_span(100, 50, 150, 75), (50, 25));
    // 5 * 0.5 = 2.5 rounds to 2; (5 + 1) * 0.5 - 1 = 2
    assert_eq!(scale_span(5, 1, 10, 5), (2, 1));
    // 3 * 0.5 = 1.5 rounds to 2, (3 + 1) * 0.5 - 1 = 1 is before it: the end follows the start
    assert_eq!(scale_span(3, 1, 10, 5), (2, 1));
    // Nothing changes when the count doesn't
    assert_eq!(scale_span(7, 3, 10, 10), (7, 3));
    // Never past the new strip
    let (start, count) = scale_span(99, 1, 100, 10);
    assert!(start + count <= 10);
}

/// The studio's own answers (`scale_segments`, from `tests/golden/hyperion/legacy.json`).
#[test]
fn scaling_equals_the_studios_for_the_golden_segments() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/golden/hyperion/legacy.json")).unwrap();
    let cases = golden["scaling"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for case in cases {
        let [start, end, old, new, want_start, want_end] =
            [0, 1, 2, 3, 4, 5].map(|i| case[i].as_u64().unwrap());
        let (got_start, got_count) = scale_span(start, end - start + 1, old, new);
        assert_eq!(
            (got_start, got_start + got_count - 1),
            (want_start, want_end),
            "segment {start}-{end} of {old} LEDs to {new}"
        );
    }
}

#[test]
fn addresses_and_manual_ranges_are_stretched_with_the_count() {
    let mut address = Address {
        channels: Vec::new(),
        count: NonZeroU64::new(150).unwrap(),
        start: 0,
    };
    scale_address(&mut address, 150, 300);
    assert_eq!((address.start, address.count.get()), (0, 300));

    let range = |cells: (u16, u64), elements: (u16, u64)| ManualRange {
        cells: Span {
            start: cells.0,
            count: NonZeroU64::new(cells.1).unwrap(),
        },
        elements: Span {
            start: elements.0,
            count: NonZeroU64::new(elements.1).unwrap(),
        },
        reversed: true,
    };
    let mut ranges = vec![range((0, 50), (0, 50)), range((50, 100), (50, 100))];
    scale_ranges(&mut ranges, 150, 300, true);
    let spans: Vec<_> = ranges
        .iter()
        .map(|r| {
            (
                r.cells.start,
                r.cells.count.get(),
                r.elements.start,
                r.elements.count.get(),
            )
        })
        .collect();
    assert_eq!(spans, [(0, 100, 0, 100), (100, 200, 100, 200)]);
    assert!(ranges.iter().all(|r| r.reversed));

    // A mapping from a canvas keeps its cells: only the target side follows the count
    let mut ranges = vec![range((10, 20), (0, 50))];
    scale_ranges(&mut ranges, 150, 300, false);
    assert_eq!(
        (ranges[0].cells.start, ranges[0].cells.count.get()),
        (10, 20)
    );
    assert_eq!(ranges[0].elements.count.get(), 100);
}
