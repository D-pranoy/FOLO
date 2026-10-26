//! The output core's behaviour: activation, the compositor's fallbacks, frames of the wrong size,
//! forgetting a source, the zone's pause, the test patterns, and the chains kept across
//! configurations.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

mod common;

use common::{STEP_US, core_of, strip_frame, strip_zone, zone};
use folo_controller::{Media, OutputCore, Pattern, PatternError};
use folo_protocol::frames::{Pixels, SurfaceFrame};
use serde_json::json;

const INSTANCE: &str = "rec";

fn bytes(out: &folo_controller::StepOutput, target: &str) -> Vec<u8> {
    let frame = &out.frames[INSTANCE];
    frame
        .targets
        .iter()
        .find(|t| t.target == target)
        .unwrap()
        .colors
        .to_bytes()
}

#[test]
fn an_inactive_zone_drives_nothing_and_reports_inactive() {
    let zone = strip_zone("z", 4, INSTANCE, &json!({"active": false}));
    let mut core = core_of(&[zone]);
    let refused = core.frame("main", "local", &strip_frame(0, &[9; 12]), 1_000);
    assert!(refused.is_err(), "no active zone has the surface");
    let out = core.step(1_000);
    assert!(out.frames.is_empty());
    assert!(out.targets.is_empty());
    let live = out.zones["z"];
    assert!(!live.active);
    assert_eq!((live.ambient, live.quiet_s), (0.0, 0.0));
}

#[test]
fn a_zone_with_no_fresh_frame_shows_its_background_times_master() {
    let extra = json!({"reactive": "on"});
    let mut z = strip_zone("z", 2, INSTANCE, &extra);
    z.surfaces[0].background = "#804020".parse().unwrap();
    z.surfaces[0].master = 0.5;
    let mut core = core_of(&[z]);
    let out = core.step(1_000);
    assert_eq!(bytes(&out, "z-strip"), [64, 32, 16, 64, 32, 16]);
    // a frame shows, and is gone again once it is 500 ms old
    core.activity("local", None, true, 2_000);
    core.frame("main", "local", &strip_frame(1, &[200; 6]), 2_000)
        .unwrap();
    assert_eq!(bytes(&core.step(2_000), "z-strip"), [200; 6]);
    assert_eq!(bytes(&core.step(502_000), "z-strip"), [200; 6]);
    assert_eq!(
        bytes(&core.step(502_001), "z-strip"),
        [64, 32, 16, 64, 32, 16]
    );
}

#[test]
fn a_forgotten_source_leaves_the_lights_at_once() {
    let mut core = core_of(&[strip_zone("z", 2, INSTANCE, &json!({}))]);
    core.activity("local", None, true, 1_000);
    core.frame("main", "local", &strip_frame(5_000, &[90; 6]), 1_000)
        .unwrap();
    assert_eq!(bytes(&core.step(1_000), "z-strip"), [90; 6]);
    core.forget_source("local");
    assert_eq!(bytes(&core.step(2_000), "z-strip"), [0; 6]);
    // a restarted engine counting from 1 is taken at once
    core.frame("main", "local", &strip_frame(1, &[30; 6]), 3_000)
        .unwrap();
    assert_eq!(bytes(&core.step(3_000), "z-strip"), [30; 6]);
}

#[test]
fn a_frame_of_another_size_is_refused_and_the_target_keeps_its_colours() {
    let mut core = core_of(&[strip_zone("z", 2, INSTANCE, &json!({}))]);
    core.frame("main", "local", &strip_frame(0, &[50; 6]), 1_000)
        .unwrap();
    assert_eq!(bytes(&core.step(1_000), "z-strip"), [50; 6]);
    let wrong = SurfaceFrame::new(0, 1, 0, 3, 1, Pixels::Rgb8(vec![1; 9]));
    assert!(core.frame("main", "local", &wrong, 2_000).is_err());
    assert_eq!(bytes(&core.step(2_000), "z-strip"), [50; 6]);
}

#[test]
fn a_zone_without_sources_shows_its_ambient_light_from_the_start() {
    let extra = json!({
        "reactive": "auto",
        "ambient": {"color": "#FF0000", "level": 1.0, "idle_after": 1.0, "style": "solid"},
    });
    let mut z = strip_zone("z", 1, INSTANCE, &extra);
    z.surfaces[0].sources.clear();
    let mut core = core_of(&[z]);
    let mut now = 0;
    core.step(now);
    // 1 s of silence, then the 1.5 s fade
    while now < 2_500_000 + STEP_US {
        now += STEP_US;
        core.step(now);
    }
    let out = core.step(now);
    assert_eq!(bytes(&out, "z-strip"), [255, 0, 0]);
    assert_eq!(out.zones["z"].ambient, 1.0);
    assert!(out.zones["z"].quiet_s > 2.5);
}

/// The zone pauses when every linked source with a media session says paused, and switches
/// after 0.3 s of silence instead of `idle_after`.
#[test]
fn a_paused_player_switches_to_the_ambient_light_after_three_tenths_of_a_second() {
    let extra = json!({
        "reactive": "auto",
        "ambient": {"color": "#00FF00", "level": 1.0, "idle_after": 60.0, "on_pause": true},
    });
    let run = |media: Option<Media>| {
        let mut core = core_of(&[strip_zone("z", 1, INSTANCE, &extra)]);
        if let Some(media) = media {
            core.media("local", media);
        }
        let mut now = 0;
        core.step(now);
        for _ in 0..30 {
            now += STEP_US;
            core.activity("local", None, false, now);
            core.step(now);
        }
        core.step(now).zones["z"].ambient
    };
    assert!(run(Some(Media::Paused)) > 0.0, "paused");
    assert_eq!(run(Some(Media::Playing)), 0.0, "playing");
    assert_eq!(run(Some(Media::NoSession)), 0.0, "no session: never paused");
    assert_eq!(run(None), 0.0, "nothing said");
}

fn line_zone(active: bool) -> folo_protocol::generated::Zone {
    zone(json!({
        "id": "room", "active": active, "reactive": "on",
        "bounds": {"x": 4.0, "y": 2.0, "z": 2.0},
        "targets": [
            {
                "id": "bar", "kind": "line",
                "provider": {"instance": INSTANCE, "address": {"start": 0, "count": 4}},
                "geometry": {"kind": "line", "count": 4, "path": [
                    {"x": -2.0, "y": -1.0, "z": 0.0}, {"x": 2.0, "y": 1.0, "z": 2.0},
                ]},
            },
            {
                "id": "loose", "kind": "line",
                "provider": {"instance": INSTANCE, "address": {"start": 4, "count": 2}},
                "geometry": {"kind": "line", "count": 2},
            },
        ],
    }))
}

#[test]
fn position_shows_where_each_element_is_in_the_zone() {
    let mut core = core_of(&[line_zone(true)]);
    let chosen = core
        .start_pattern("room", &[], Pattern::Position, 5.0, 0)
        .unwrap();
    assert_eq!(chosen, ["bar", "loose"]);
    let out = core.step(0);
    // corner to corner of the box: 0, 1/3, 2/3 and 1 on every axis
    assert_eq!(
        bytes(&out, "bar"),
        [0, 0, 0, 85, 85, 85, 170, 170, 170, 255, 255, 255]
    );
    // an unplaced path is dark
    assert_eq!(bytes(&out, "loose"), [0; 6]);
    assert_eq!(out.targets["bar"], bytes(&out, "bar"));
}

#[test]
fn sweep_lights_one_element_at_a_time_and_identify_pulses() {
    let mut core = core_of(&[line_zone(true)]);
    core.start_pattern("room", &["bar".to_owned()], Pattern::Sweep, 5.0, 0)
        .unwrap();
    let lit = |out: &folo_controller::StepOutput| {
        bytes(out, "bar")
            .chunks(3)
            .map(|c| c == [63, 63, 63])
            .collect::<Vec<_>>()
    };
    assert_eq!(lit(&core.step(0)), [true, false, false, false]);
    assert_eq!(lit(&core.step(500_000)), [false, true, false, false]);
    assert_eq!(lit(&core.step(1_999_999)), [false, false, false, true]);
    assert_eq!(lit(&core.step(2_000_000)), [true, false, false, false]);

    core.start_pattern("room", &["bar".to_owned()], Pattern::Identify, 5.0, 0)
        .unwrap();
    assert_eq!(bytes(&core.step(0), "bar"), [0; 12]);
    assert_eq!(bytes(&core.step(500_000), "bar"), [63; 12]);
    assert_eq!(bytes(&core.step(1_000_000), "bar"), [0; 12]);
}

#[test]
fn a_pattern_runs_on_an_inactive_zone_and_ends_by_itself() {
    let mut core = core_of(&[line_zone(false)]);
    assert!(core.step(0).frames.is_empty());
    core.start_pattern("room", &["bar".to_owned()], Pattern::Identify, 1.0, 0)
        .unwrap();
    let out = core.step(500_000);
    assert_eq!(out.frames[INSTANCE].targets.len(), 1);
    assert!(!out.zones["room"].active);
    // ended: nothing drives the target, so its instance gets nothing (and is released)
    assert!(core.step(1_000_000).frames.is_empty());
}

#[test]
fn patterns_refuse_what_the_zone_doesnt_have() {
    let mut core = core_of(&[line_zone(true)]);
    assert_eq!(
        core.start_pattern("nope", &[], Pattern::Sweep, 5.0, 0),
        Err(PatternError::UnknownZone)
    );
    assert_eq!(
        core.start_pattern("room", &["bar".into(), "x".into()], Pattern::Sweep, 5.0, 0),
        Err(PatternError::UnknownTarget("x".into()))
    );
    // nothing started
    assert_eq!(bytes(&core.step(0), "bar"), [0; 12]);
}

/// The flash limiter's history outlives a configuration that leaves the target's settings as
/// they were: an edit elsewhere in the zones can't let a fourth flash through.
#[test]
fn a_new_revision_keeps_the_flash_limiters_history() {
    let extra = json!({"flash_limiter": {"enabled": true}});
    let zones = [strip_zone("z", 1, INSTANCE, &extra)];
    let mut core = core_of(&zones);
    let mut edges = 0;
    let mut last = None;
    // 10 Hz black and white for one second; a new revision halfway
    for i in 0..60u32 {
        let now = 1_000_000 + u64::from(i) * STEP_US;
        if i == 30 {
            core.configure(2, &zones);
        }
        let value = if (i / 3) % 2 == 0 { 255 } else { 0 };
        core.activity("local", None, true, now);
        core.frame("main", "local", &strip_frame(i, &[value; 3]), now)
            .unwrap();
        let shown = bytes(&core.step(now), "z-strip")[0];
        if last.is_some_and(|l| l != shown) {
            edges += 1;
        }
        last = Some(shown);
    }
    // three flashes are six edges; the release after the window may add one
    assert!(edges <= 7, "{edges} edges in a second");
}

#[test]
fn display_targets_and_targets_without_an_instance_are_not_sent() {
    let z = zone(json!({
        "id": "z",
        "targets": [
            {"id": "screen", "kind": "display", "provider": {"instance": INSTANCE}, "geometry": {"kind": "display"}},
            {"id": "spare", "kind": "point", "geometry": {"kind": "point"}},
        ],
    }));
    let mut core = core_of(&[z]);
    let out = core.step(0);
    assert!(out.frames.is_empty());
    assert!(!out.targets.contains_key("screen"));
    // a target bound to no instance still shows on its topic
    assert!(out.targets.contains_key("spare"));
}

/// A rough measure of one step on the parity path, for the 0.8 ms budget (ADR-0036): a 300-LED
/// strip, in this build profile. It asserts nothing about time, only prints it.
#[test]
fn a_step_of_a_three_hundred_led_strip_is_measured() {
    let mut core: OutputCore = core_of(&[strip_zone("z", 300, INSTANCE, &json!({}))]);
    let row: Vec<u8> = (0..900).map(|i| (i % 256) as u8).collect();
    let steps = 2_000u32;
    let start = std::time::Instant::now();
    for i in 0..steps {
        let now = 1_000_000 + u64::from(i) * STEP_US;
        core.activity("local", None, true, now);
        core.frame("main", "local", &strip_frame(i, &row), now)
            .unwrap();
        std::hint::black_box(core.step(now));
    }
    let per_step = start.elapsed() / steps;
    eprintln!("one step of a 300-LED strip: {per_step:?}");
}
