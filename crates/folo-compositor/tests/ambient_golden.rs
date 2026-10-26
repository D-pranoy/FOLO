//! The zone ambient light against the current engine's ambient frames
//! (RB-08 step 3, `docs/architecture/pipeline.md#parity-mode`): within ±1 per
//! channel of `tests/golden/renderer/ambient-*.png`, which the legacy
//! renderer wrote and these tests only read.
//!
//! The inputs are `tests/golden/ambient/` (`scripts/make_golden.py --only
//! ambient`): the reactive frames with `master` applied and truncated to bytes,
//! as the engine sends them, plus each step's seconds and whether the audio was
//! active. `step` (seconds) is compared with the legacy frames. `step_to` (a
//! clock in whole microseconds) must agree with `step` fed the same rounded
//! seconds: a clock can't be asked to land the legacy path's float sums on the
//! same side of a threshold, and a switch one frame early or late moves a fade
//! by a few steps.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use folo_compositor::{AmbientSettings, AmbientStyle, Reactive, ZoneAmbient};
use serde::Deserialize;

/// Per channel, as the pipeline document says.
const TOLERANCE: i16 = 1;

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

#[derive(Deserialize)]
struct Manifest {
    cases: std::collections::BTreeMap<String, Case>,
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
    frames: usize,
    leds: usize,
    dts: Vec<f64>,
    active: String,
}

struct Image {
    width: usize,
    rows: Vec<Vec<u8>>,
}

fn read_png(path: &Path) -> Image {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight),
        "{}",
        path.display()
    );
    let stride = info.line_size;
    Image {
        width: info.width as usize,
        rows: buffer[..stride * info.height as usize]
            .chunks_exact(stride)
            .map(<[u8]>::to_vec)
            .collect(),
    }
}

fn settings(case: &Case) -> AmbientSettings {
    let hex = case.ambient.color.trim_start_matches('#');
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap();
    AmbientSettings {
        reactive: match case.reactive.as_str() {
            "on" => Reactive::On,
            "auto" => Reactive::Auto,
            "off" => Reactive::Off,
            other => panic!("reactive {other}"),
        },
        color: [byte(0), byte(2), byte(4)],
        style: match case.ambient.style.as_str() {
            "solid" => AmbientStyle::Solid,
            "breathe" => AmbientStyle::Breathe,
            other => panic!("style {other}"),
        },
        level: case.ambient.level,
        idle_after_s: case.ambient.idle_after,
        on_pause: case.on_pause,
    }
}

struct Outcome {
    biggest: i16,
    differing: usize,
    changed_by_the_light: usize,
    brighter: usize,
}

fn input_of(case: &Case) -> (Image, Image) {
    let input = read_png(&golden().join("ambient").join(&case.input));
    let expected = read_png(&golden().join(&case.expected));
    assert_eq!(input.width, case.leds);
    assert_eq!(
        (input.rows.len(), expected.rows.len()),
        (case.frames, case.frames)
    );
    assert_eq!(
        (case.dts.len(), case.active.len()),
        (case.frames, case.frames)
    );
    assert_eq!(input.width, expected.width);
    (input, expected)
}

fn float_row(row: &[u8]) -> Vec<f32> {
    row.iter().map(|&v| f32::from(v)).collect()
}

/// Steps in the legacy path's own seconds and compares every frame with its PNG.
fn against_legacy(case: &Case) -> Outcome {
    let (input, expected) = input_of(case);
    let mut ambient = ZoneAmbient::new(settings(case));
    let mut outcome = Outcome {
        biggest: 0,
        differing: 0,
        changed_by_the_light: 0,
        brighter: 0,
    };
    for (i, (row, want)) in input.rows.iter().zip(&expected.rows).enumerate() {
        ambient.step(case.dts[i], case.active.as_bytes()[i] == b'1', case.paused);
        let mut pixels = float_row(row);
        ambient.apply(&mut pixels, case.master);
        let mut differs = false;
        for (got, (&want, &input)) in pixels.iter().zip(want.iter().zip(row)) {
            outcome.brighter += usize::from(*got as i16 > i16::from(want));
            let diff = (*got as i16 - i16::from(want)).abs();
            outcome.biggest = outcome.biggest.max(diff);
            differs |= diff != 0;
            if *got as u8 != input {
                outcome.changed_by_the_light += 1;
            }
        }
        outcome.differing += usize::from(differs);
    }
    outcome
}

/// `step_to` on a clock that ticks the golden steps in whole microseconds
/// against `step` given those seconds: the same frames, bit for bit.
fn clock_against_seconds(case: &Case) -> usize {
    let (input, _) = input_of(case);
    let mut seconds = ZoneAmbient::new(settings(case));
    let mut timed = ZoneAmbient::new(settings(case));
    let mut now_us = 1_000_000_u64;
    let mut lit = 0;
    for (i, row) in input.rows.iter().enumerate() {
        let active = case.active.as_bytes()[i] == b'1';
        // the first call only sets the clock
        let dt_us = if i == 0 {
            0
        } else {
            (case.dts[i] * 1e6).round() as u64
        };
        now_us += dt_us;
        seconds.step(dt_us as f64 / 1e6, active, case.paused);
        timed.step_to(now_us, active, case.paused);
        let (mut a, mut b) = (float_row(row), float_row(row));
        seconds.apply(&mut a, case.master);
        timed.apply(&mut b, case.master);
        assert_eq!(a, b, "frame {i}");
        lit += usize::from(a != float_row(row));
    }
    lit
}

/// How far a fade that starts one step off may be from the legacy frame. The
/// eased fade-in moves at most 255 × (1 / 1.5 s) × 1.5 = 255 levels a second
/// (the slope of the ease is 1.5 at its middle), so one idle step (1/30 s) moves it by
/// 8.5 levels, and the ±1 of the maths and the truncation come on top.
const CLOCK_TOLERANCE: i16 = 11;

/// `step_to` on a clock in whole microseconds against the legacy frames: the
/// biggest difference. The clock can't land the legacy float sums on the same
/// side of a threshold (`quiet >= idle` at exactly 0.3 s in one case: legacy
/// 0.3, the clock's 0.2999997 a step before), so a fade may start one step
/// early or late, and that is all [`CLOCK_TOLERANCE`] allows for.
fn clock_against_legacy(case: &Case) -> i16 {
    let (input, expected) = input_of(case);
    let mut timed = ZoneAmbient::new(settings(case));
    let mut now_us = 1_000_000_u64;
    // the first call only sets the clock: prime it, so the first step counts like the others
    timed.step_to(now_us, false, case.paused);
    let mut biggest = 0;
    for (i, (row, want)) in input.rows.iter().zip(&expected.rows).enumerate() {
        now_us += (case.dts[i] * 1e6).round() as u64;
        timed.step_to(now_us, case.active.as_bytes()[i] == b'1', case.paused);
        let mut pixels = float_row(row);
        timed.apply(&mut pixels, case.master);
        for (&got, &want) in pixels.iter().zip(want) {
            biggest = biggest.max((got as i16 - i16::from(want)).abs());
        }
    }
    biggest
}

fn manifest() -> Manifest {
    serde_json::from_slice(
        &fs::read(golden().join("ambient/manifest.json"))
            .expect("tests/golden/ambient/manifest.json"),
    )
    .unwrap()
}

#[test]
fn the_zone_ambient_light_matches_the_legacy_frames_within_one() {
    let manifest = manifest();
    assert_eq!(manifest.cases.len(), 9);
    for (name, case) in &manifest.cases {
        let outcome = against_legacy(case);
        eprintln!(
            "{name}: {} of {} frames differ from the legacy frame, by at most {}",
            outcome.differing, case.frames, outcome.biggest
        );
        assert!(
            outcome.biggest <= TOLERANCE,
            "{name}: off by {} (more than {TOLERANCE})",
            outcome.biggest
        );
        // the engine's frame was truncated before the light came on, so the result
        // is never above the legacy one; rounding instead of truncating would be
        assert_eq!(
            outcome.brighter, 0,
            "{name}: brighter than the legacy frame"
        );
        // the cases are only a test if the light does something
        assert!(
            outcome.changed_by_the_light > 0,
            "{name}: the ambient light never showed"
        );
    }
}

#[test]
fn a_clock_steps_the_light_like_seconds_do() {
    for (name, case) in &manifest().cases {
        assert!(
            clock_against_seconds(case) > 0,
            "{name}: the ambient light never showed"
        );
    }
}

#[test]
fn a_clock_keeps_to_the_legacy_frames_give_or_take_a_step() {
    for (name, case) in &manifest().cases {
        let biggest = clock_against_legacy(case);
        eprintln!("{name}: on a clock, off by at most {biggest}");
        assert!(
            biggest <= CLOCK_TOLERANCE,
            "{name}: off by {biggest} (more than {CLOCK_TOLERANCE})"
        );
    }
}

#[test]
fn on_never_changes_a_frame() {
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(golden().join("ambient/manifest.json")).unwrap()).unwrap();
    let (name, case) = manifest.cases.iter().next().unwrap();
    let input = read_png(&golden().join("ambient").join(&case.input));
    let mut ambient = ZoneAmbient::new(AmbientSettings {
        reactive: Reactive::On,
        ..settings(case)
    });
    for (i, row) in input.rows.iter().enumerate() {
        ambient.step(case.dts[i], false, true);
        let mut pixels: Vec<f32> = row.iter().map(|&v| f32::from(v)).collect();
        ambient.apply(&mut pixels, case.master);
        assert!(
            pixels.iter().zip(row).all(|(&got, &v)| got == f32::from(v)),
            "{name} frame {i}"
        );
    }
}
