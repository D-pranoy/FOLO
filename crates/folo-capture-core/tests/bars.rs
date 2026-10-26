//! The black-bar detector (ADR-0037 D7, D22): each mode against what
//! Hyperion.NG's detector gives on hand-worked frames, then the golden
//! scenarios of RB-14 step 9 on synthetic frames, with the bars and the times
//! they change checked exactly. The frames are made here, so no fixture files.

use folo_capture_core::bars::BarDetector;
use folo_capture_core::bars::hysteresis::{Hysteresis, Timing};
use folo_capture_core::bars::scan::{Border, Image, Scanner};
use folo_capture_core::synthetic::{Pattern, SyntheticBackend};
use folo_capture_core::types::{
    BarFractions, Bars, BlackBarMode, BlackBarPreset, BlackBarSettings, FrameFlags, Profile,
};

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];
const PICTURE: [u8; 3] = [200, 120, 60];
const MS: u64 = 1000;

/// A frame whose pixel at (x, y) is `paint(x, y)`.
fn frame(width: u16, height: u16, mut paint: impl FnMut(u16, u16) -> [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&paint(x, y));
        }
    }
    pixels
}

/// Lights the cells `x0..x1` × `y0..y1` of a frame `width` cells wide.
fn light(pixels: &mut [u8], width: u16, (x0, x1): (u16, u16), (y0, y1): (u16, u16)) {
    for y in y0..y1 {
        for x in x0..x1 {
            let at = (usize::from(y) * usize::from(width) + usize::from(x)) * 3;
            pixels[at..at + 3].copy_from_slice(&WHITE);
        }
    }
}

/// A picture in the rectangle `x0..x1` × `y0..y1` on black.
fn boxed(width: u16, height: u16, x0: u16, x1: u16, y0: u16, y1: u16) -> Vec<u8> {
    frame(width, height, |x, y| {
        if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
            PICTURE
        } else {
            BLACK
        }
    })
}

fn scan(mode: BlackBarMode, width: u16, height: u16, pixels: &[u8]) -> Border {
    let image = Image::new(width, height, pixels).unwrap();
    Scanner::new(0.05, 0).scan(mode, &image)
}

fn known(rows: usize, columns: usize) -> Border {
    Border {
        unknown: false,
        rows,
        columns,
    }
}

fn settings(preset: BlackBarPreset, mode: BlackBarMode) -> BlackBarSettings {
    BlackBarSettings {
        preset,
        mode,
        threshold: 0.05,
    }
}

fn auto(mode: BlackBarMode) -> BlackBarSettings {
    settings(BlackBarPreset::Auto, mode)
}

fn bars(top_bottom: u16, left_right: u16) -> Bars {
    Bars {
        top: top_bottom,
        bottom: top_bottom,
        left: left_right,
        right: left_right,
    }
}

/// Feeds `count` frames `step_us` apart from `start_us`; the bars after each.
fn feed(
    detector: &mut BarDetector,
    (width, height): (u16, u16),
    pixels: &[u8],
    start_us: u64,
    step_us: u64,
    count: u64,
) -> Vec<Bars> {
    (0..count)
        .map(|index| detector.update(width, height, pixels, start_us + index * step_us))
        .collect()
}

/// The index of the first element of `series` equal to `wanted`.
fn first_at(series: &[Bars], wanted: Bars) -> Option<usize> {
    series.iter().position(|&seen| seen == wanted)
}

// What Hyperion.NG's detector gives, worked by hand ------------------------

/// 30 × 30, the picture in columns 6..24 and rows 4..26: black 4 rows and 6
/// columns on each side. Hyperion.NG's `process` samples columns x = 0, 1, …
/// at (29 − x, 15), (x, 10) and (x, 20), and rows y = 0, 1, … at (15, 29 − y),
/// (10, y) and (20, y): the first non-black is at x = 6 and y = 4.
#[test]
fn default_mode_finds_the_first_cell_of_the_picture_on_its_lines() {
    let pixels = boxed(30, 30, 6, 24, 4, 26);
    assert_eq!(scan(BlackBarMode::Default, 30, 30, &pixels), known(4, 6));
}

/// The right and bottom bars are looked at through the centre only, so a
/// picture that is black at the left but not the right reads the right's.
#[test]
fn default_mode_reads_the_right_and_bottom_through_the_centre_lines() {
    // The picture reaches column 22 on the right: (29 − x, 15) is non-black
    // at x = 7, but (x, 10) is black until the picture starts at column 9.
    let pixels = boxed(30, 30, 9, 23, 4, 26);
    assert_eq!(scan(BlackBarMode::Default, 30, 30, &pixels).columns, 7);
    // The bottom: the picture reaches row 22; (15, 29 − y) is non-black at
    // y = 7, (10, y) and (20, y) only at y = 8.
    let pixels = boxed(30, 30, 6, 24, 8, 23);
    assert_eq!(scan(BlackBarMode::Default, 30, 30, &pixels).rows, 7);
}

#[test]
fn a_frame_with_nothing_on_its_lines_is_unknown() {
    let black = boxed(30, 30, 0, 0, 0, 0);
    assert!(scan(BlackBarMode::Default, 30, 30, &black).unknown);
    assert!(scan(BlackBarMode::Letterbox, 30, 30, &black).unknown);
    assert!(scan(BlackBarMode::Osd, 30, 30, &black).unknown);
}

/// One axis not found makes the border unknown, with the other as found
/// (Hyperion.NG sets `unknown` from either index being −1).
#[test]
fn default_mode_is_unknown_when_one_axis_finds_nothing() {
    // Only the bottom centre pixel is lit: the rows end at y = 0, no column does.
    let mut pixels = boxed(30, 30, 0, 0, 0, 0);
    let at = (29 * 30 + 15) * 3;
    pixels[at..at + 3].copy_from_slice(&WHITE);
    let border = scan(BlackBarMode::Default, 30, 30, &pixels);
    assert!(border.unknown);
    assert_eq!(border.rows, 0);
}

/// No bars at all is a border of 0 and 0, not unknown.
#[test]
fn a_picture_to_the_edges_has_no_bars() {
    let pixels = boxed(30, 30, 0, 30, 0, 30);
    assert_eq!(scan(BlackBarMode::Default, 30, 30, &pixels), known(0, 0));
}

/// The letterbox mode looks at the top and bottom only (columns are 0): at
/// x = 20 (centre) and 10 and 30 (a quarter, three quarters) of a 40-wide
/// frame at the top, and at 10 and 30 at the bottom.
#[test]
fn letterbox_mode_finds_rows_and_leaves_the_columns_at_0() {
    let pixels = boxed(40, 30, 8, 32, 5, 25);
    assert_eq!(scan(BlackBarMode::Letterbox, 40, 30, &pixels), known(5, 0));
}

/// A subtitle in the bottom bar at the centre doesn't count in letterbox
/// mode, which doesn't sample the centre there, and counts in default mode.
#[test]
fn letterbox_mode_ignores_a_caption_at_the_bottom_centre_that_default_mode_reads() {
    let mut pixels = boxed(40, 30, 0, 40, 5, 25);
    light(&mut pixels, 40, (16, 24), (27, 29));
    assert_eq!(scan(BlackBarMode::Letterbox, 40, 30, &pixels).rows, 5);
    // Default samples (20, 29 − y): the caption is hit at y = 1.
    assert_eq!(scan(BlackBarMode::Default, 40, 30, &pixels).rows, 1);
}

/// The OSD mode finds the columns, then looks for rows at that column (and
/// at its mirror) only, so an overlay at the first third column of the top
/// bar, which the default mode samples, doesn't shorten the rows.
#[test]
fn osd_mode_looks_for_rows_at_the_column_it_found() {
    let mut pixels = boxed(30, 30, 6, 24, 4, 26);
    let at = (30 + 10) * 3; // (10, 1)
    pixels[at..at + 3].copy_from_slice(&WHITE);
    assert_eq!(scan(BlackBarMode::Default, 30, 30, &pixels), known(1, 6));
    assert_eq!(scan(BlackBarMode::Osd, 30, 30, &pixels), known(4, 6));
}

/// Without a column found, Hyperion.NG's counter ends at a third of the
/// width and the row scan runs at that column.
#[test]
fn osd_mode_scans_rows_at_a_third_of_the_width_when_no_column_was_found() {
    // Lit pixel at (10, 2), a third of the width: rows end at 2, columns unknown.
    let mut pixels = boxed(30, 30, 0, 0, 0, 0);
    let at = (2 * 30 + 10) * 3;
    pixels[at..at + 3].copy_from_slice(&WHITE);
    let border = scan(BlackBarMode::Osd, 30, 30, &pixels);
    assert!(border.unknown);
    assert_eq!(border.rows, 2);
}

/// `ceil(threshold × 255)`: 5% is 13, so 12 is black and 13 isn't.
#[test]
fn a_pixel_is_black_when_every_channel_is_below_ceil_threshold_times_255() {
    let dark = frame(30, 30, |_, _| [12, 12, 12]);
    assert!(scan(BlackBarMode::Default, 30, 30, &dark).unknown);
    let grey = frame(30, 30, |_, _| [12, 12, 13]);
    assert_eq!(scan(BlackBarMode::Default, 30, 30, &grey), known(0, 0));
    // 0.01 → 3 (2.55 rounded up), 0.5 → 128 (127.5 rounded up).
    let low = frame(30, 30, |_, _| [2, 2, 2]);
    let at_3 = frame(30, 30, |_, _| [3, 3, 3]);
    let scanner = Scanner::new(0.01, 0);
    assert!(
        scanner
            .scan(BlackBarMode::Default, &Image::new(30, 30, &low).unwrap())
            .unknown
    );
    assert!(
        !scanner
            .scan(BlackBarMode::Default, &Image::new(30, 30, &at_3).unwrap())
            .unknown
    );
    let half = frame(30, 30, |_, _| [127, 127, 127]);
    let at_128 = frame(30, 30, |_, _| [128, 128, 128]);
    let scanner = Scanner::new(0.5, 0);
    assert!(
        scanner
            .scan(BlackBarMode::Default, &Image::new(30, 30, &half).unwrap())
            .unknown
    );
    assert!(
        !scanner
            .scan(BlackBarMode::Default, &Image::new(30, 30, &at_128).unwrap())
            .unknown
    );
}

#[test]
fn an_image_needs_cells_and_exactly_three_bytes_each() {
    assert!(Image::new(0, 4, &[]).is_none());
    assert!(Image::new(2, 2, &[0; 11]).is_none());
    assert!(Image::new(2, 2, &[0; 12]).is_some());
}

/// Frames too small for a line to run find nothing and don't panic.
#[test]
fn tiny_frames_are_unknown_in_every_mode() {
    for (width, height) in [(1u16, 1u16), (2, 2), (3, 1), (1, 7), (5, 2)] {
        let pixels = vec![255u8; usize::from(width) * usize::from(height) * 3];
        for mode in [
            BlackBarMode::Default,
            BlackBarMode::Letterbox,
            BlackBarMode::Osd,
        ] {
            let _ = scan(mode, width, height, &pixels);
        }
    }
    assert!(scan(BlackBarMode::Default, 2, 2, &[255; 12]).unknown);
}

// The hysteresis, worked by hand ------------------------------------------

fn at(seconds_tenths: u64) -> u64 {
    seconds_tenths * 100 * MS
}

#[test]
fn the_first_border_found_is_adopted_at_once() {
    let mut hysteresis = Hysteresis::new(Timing::of(Profile::Video));
    assert!(hysteresis.update(known(5, 0), at(0)));
    assert_eq!(hysteresis.current(), known(5, 0));
}

/// A new border is adopted after being seen steadily for 2 s, once the
/// first 0.4 s of differing frames, which are tolerated, have gone by.
#[test]
fn a_new_border_waits_out_the_tolerance_and_then_the_hold() {
    let mut hysteresis = Hysteresis::new(Timing::of(Profile::Video));
    // The second frame settles the first border: until a frame matches the
    // one being watched, a differing frame replaces it at once, as in
    // Hyperion.NG.
    hysteresis.update(known(5, 0), 0);
    hysteresis.update(known(5, 0), at(1));
    // Seen from 1.0 s; 0.4 s are tolerated (the 1.4 s frame still is), the
    // 1.5 s frame is the first that is watched, and it is adopted 2 s later.
    for tenths in 10..=14 {
        assert!(!hysteresis.update(known(8, 0), at(tenths)));
    }
    for tenths in 15..=34 {
        assert!(!hysteresis.update(known(8, 0), at(tenths)), "at {tenths}");
    }
    assert!(hysteresis.update(known(8, 0), at(35)));
    assert_eq!(hysteresis.current(), known(8, 0));
}

#[test]
fn a_few_odd_frames_are_ignored_and_do_not_restart_the_hold() {
    let mut hysteresis = Hysteresis::new(Timing::of(Profile::Video));
    hysteresis.update(known(5, 0), 0);
    for tenths in 1..=30 {
        // A bad frame with a smaller border every second.
        let seen = if tenths % 10 == 0 {
            known(2, 0)
        } else {
            known(5, 0)
        };
        assert!(!hysteresis.update(seen, at(tenths)));
    }
    assert_eq!(hysteresis.current(), known(5, 0));
}

#[test]
fn unknown_is_adopted_after_24_seconds_and_a_known_border_after_unknown_at_once() {
    let mut hysteresis = Hysteresis::new(Timing::of(Profile::Video));
    hysteresis.update(known(5, 0), 0);
    hysteresis.update(known(5, 0), at(1));
    // Unknown from 1 s: tolerated until 1.4 s, watched from 1.5 s.
    for tenths in (10..=15).chain((16..=254).step_by(1)) {
        assert!(
            !hysteresis.update(Border::UNKNOWN, at(tenths)),
            "at {tenths}"
        );
    }
    assert!(hysteresis.update(Border::UNKNOWN, at(255)));
    assert!(hysteresis.current().unknown);
    // A border after unknown needs no hold, only the tolerance: the frames
    // of 25.6 to 26.0 s are tolerated and the one at 26.1 s is adopted.
    for tenths in 256..=260 {
        assert!(!hysteresis.update(known(6, 0), at(tenths)), "at {tenths}");
    }
    assert!(hysteresis.update(known(6, 0), at(261)));
    assert_eq!(hysteresis.current(), known(6, 0));
}

/// Hyperion.NG's unknown matches unknown whatever the sizes.
#[test]
fn unknown_borders_are_the_same_whatever_their_sizes() {
    let one = Border {
        unknown: true,
        rows: 3,
        columns: 0,
    };
    let other = Border {
        unknown: true,
        rows: 0,
        columns: 9,
    };
    assert!(one.same_as(&other));
    assert!(!one.same_as(&known(3, 0)));
    assert!(!known(3, 0).same_as(&one));
}

// The golden scenarios (RB-14 step 9) --------------------------------------

const W: u16 = 64;
const H: u16 = 36;
const SIZE: (u16, u16) = (W, H);
/// 25 frames a second, the rate Hyperion.NG's counts are for.
const FRAME: u64 = 40 * MS;

fn video() -> BarDetector {
    BarDetector::new(auto(BlackBarMode::Default), Profile::Video)
}

/// The picture of a film with 5 black rows on each side, in a 64 × 36 frame.
fn letterbox_picture() -> Vec<u8> {
    boxed(W, H, 0, W, 5, 31)
}

#[test]
fn letterbox_is_found_with_the_blur_margin_on_the_first_frame() {
    let mut detector = video();
    let found = feed(&mut detector, SIZE, &letterbox_picture(), 0, FRAME, 3);
    // 5 rows found, plus 1 for the blur; no columns.
    assert_eq!(found, vec![bars(6, 0); 3]);
}

#[test]
fn pillarbox_is_found_in_columns() {
    let pixels = boxed(W, H, 8, 56, 0, H);
    let mut detector = video();
    let found = feed(&mut detector, SIZE, &pixels, 0, FRAME, 3);
    assert_eq!(
        found[2],
        Bars {
            top: 0,
            bottom: 0,
            left: 9,
            right: 9
        }
    );
}

#[test]
fn a_21_to_9_film_on_a_16_to_9_screen_is_letterboxed() {
    // 64 / 28 is about 2.29 : 1.
    let pixels = boxed(W, H, 0, W, 4, 32);
    let mut detector = video();
    assert_eq!(
        feed(&mut detector, SIZE, &pixels, 0, FRAME, 2)[1],
        bars(5, 0)
    );
}

#[test]
fn a_picture_that_fills_the_frame_has_no_bars() {
    let pixels = boxed(W, H, 0, W, 0, H);
    let mut detector = video();
    assert_eq!(
        feed(&mut detector, SIZE, &pixels, 0, FRAME, 2)[1],
        Bars::default()
    );
}

/// Default mode samples the bottom centre, so a subtitle there ends its
/// scan early, and after being steady for the hold it is adopted; letterbox
/// mode doesn't sample there, and never moves.
#[test]
fn a_subtitle_in_the_bars_fools_the_default_mode_after_the_hold_but_not_letterbox() {
    let mut captioned = letterbox_picture();
    // Rows 32 and 33, columns 24..40: the bottom centre (32, 35 − y) at y = 2 and 3.
    light(&mut captioned, W, (24, 40), (32, 34));
    let plain = letterbox_picture();

    let mut default = video();
    feed(&mut default, SIZE, &plain, 0, FRAME, 10);
    let found = feed(&mut default, SIZE, &captioned, 10 * FRAME, FRAME, 100);
    // Rows 5 → the subtitle ends the scan at y = 2, plus the blur: 3. Seen
    // from frame 0 of this run (0.4 s), tolerated for 0.4 s (11 frames),
    // watched from frame 11 and adopted 2 s (50 frames) later, at frame 61.
    assert_eq!(first_at(&found, bars(3, 0)), Some(61));
    assert_eq!(found[60], bars(6, 0));

    let mut letterbox = BarDetector::new(auto(BlackBarMode::Letterbox), Profile::Video);
    feed(&mut letterbox, SIZE, &plain, 0, FRAME, 10);
    let found = feed(&mut letterbox, SIZE, &captioned, 10 * FRAME, FRAME, 100);
    assert!(found.iter().all(|&seen| seen == bars(6, 0)));
}

/// A subtitle that shows for a second, as in a film, is shorter than the
/// 2 s hold and the bars never change.
#[test]
fn a_short_subtitle_never_changes_the_bars() {
    let mut captioned = letterbox_picture();
    light(&mut captioned, W, (24, 40), (32, 34));
    let mut detector = video();
    let mut all = feed(&mut detector, SIZE, &letterbox_picture(), 0, FRAME, 10);
    all.extend(feed(&mut detector, SIZE, &captioned, 10 * FRAME, FRAME, 25));
    all.extend(feed(
        &mut detector,
        SIZE,
        &letterbox_picture(),
        35 * FRAME,
        FRAME,
        100,
    ));
    assert!(all.iter().all(|&seen| seen == bars(6, 0)));
}

/// A channel logo in the corner of the bars isn't on a sampled line.
#[test]
fn a_logo_in_a_corner_of_the_bars_is_ignored() {
    let mut logo = letterbox_picture();
    light(&mut logo, W, (2, 8), (1, 4));
    let mut detector = video();
    let found = feed(&mut detector, SIZE, &logo, 0, FRAME, 200);
    assert!(found.iter().all(|&seen| seen == bars(6, 0)));
}

/// The picture fades to black: the bars stay while it fades, and the unknown
/// border (no bars) is adopted only after a long time of black: 0.4 s
/// tolerated, then 24 s, from the first black frame.
#[test]
fn a_fade_to_black_keeps_the_bars_until_24_seconds_of_black() {
    let mut detector = video();
    feed(&mut detector, SIZE, &letterbox_picture(), 0, FRAME, 10);
    let mut seen = Vec::new();
    for step in 0..=25u8 {
        // Brightness steps down by 8 per frame: the picture at 200 is dark by step 25.
        let level = 200u8.saturating_sub(step * 8);
        let pixels = frame(W, H, |_, y| {
            if (5..31).contains(&y) {
                [level; 3]
            } else {
                BLACK
            }
        });
        seen.push(detector.update(W, H, &pixels, (10 + u64::from(step)) * FRAME));
    }
    assert!(seen.iter().all(|&bars_seen| bars_seen == bars(6, 0)));

    let black = boxed(W, H, 0, 0, 0, 0);
    let start = 40 * FRAME;
    let found = feed(&mut detector, SIZE, &black, start, FRAME, 650);
    // The picture is below the threshold (13) on its lines from step 24, 6
    // frames before the black ones. Unknown differs from then, is tolerated
    // for 11 frames (0.44 s is the first beyond 0.4 s), and is adopted 600
    // frames (24 s) after that: at frame 11 + 600 − 6 of the black ones.
    let adopted = first_at(&found, Bars::default()).expect("adopted");
    assert!(found[..adopted].iter().all(|&seen| seen == bars(6, 0)));
    assert_eq!(adopted, 11 + 600 - 6);
}

/// 5 black rows at the top and bottom of the frame, with an on-screen display
/// (a volume bar) in the top bar at a column the default mode samples.
#[test]
fn an_osd_overlay_shortens_the_default_mode_bars_and_not_the_osd_mode() {
    let mut overlay = letterbox_picture();
    // Columns W/3 = 21: (21, y) for y 0..12 is on the default mode's lines.
    light(&mut overlay, W, (20, 24), (1, 3));
    let mut default = video();
    assert_eq!(
        feed(&mut default, SIZE, &overlay, 0, FRAME, 1)[0],
        bars(2, 0)
    );
    let mut osd = BarDetector::new(auto(BlackBarMode::Osd), Profile::Video);
    // The columns come out at 0 (the picture spans the width), so the rows
    // are read at column 0 and 63, where the overlay isn't.
    assert_eq!(feed(&mut osd, SIZE, &overlay, 0, FRAME, 1)[0], bars(6, 0));
}

/// A little noise in the black bars, below the threshold, doesn't matter;
/// noise at the threshold ends the scan.
#[test]
fn noise_below_the_threshold_is_black_and_at_the_threshold_is_not() {
    let mut seed = 12345u32;
    let mut noise = move |top: u8| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        u8::try_from((seed >> 24) % (u32::from(top) + 1)).unwrap()
    };
    let mut detector = video();
    for index in 0..100u64 {
        let pixels = frame(W, H, |_, y| {
            if (5..31).contains(&y) {
                PICTURE
            } else {
                [noise(12), noise(12), noise(12)]
            }
        });
        let found = detector.update(W, H, &pixels, index * FRAME);
        assert_eq!(found, bars(6, 0), "frame {index}");
    }
    // One pixel of 13 on the centre line, at row 2: one frame of it is a blip.
    let mut blip = letterbox_picture();
    let at = (2 * usize::from(W) + 32) * 3;
    blip[at] = 13;
    let found = detector.update(W, H, &blip, 100 * FRAME);
    assert_eq!(found, bars(6, 0));
    assert_eq!(scan(BlackBarMode::Default, W, H, &blip).rows, 5);
    // Down to 12, it's black again and the scan runs on to the picture.
    blip[at] = 12;
    assert_eq!(scan(BlackBarMode::Default, W, H, &blip).rows, 5);
}

/// The same scene at 25, 60 and 100 frames a second changes after the same
/// time, within a frame: hysteresis counts time.
#[test]
fn the_time_to_adopt_a_border_does_not_depend_on_the_frame_rate() {
    for frame_us in [10 * MS, 40 * MS, 1_000_000 / 60] {
        let mut detector = video();
        feed(&mut detector, SIZE, &letterbox_picture(), 0, frame_us, 2);
        let wide = boxed(W, H, 0, W, 8, 28);
        let found = feed(&mut detector, SIZE, &wide, 1_000_000, frame_us, 400);
        let index = first_at(&found, bars(9, 0)).expect("adopted");
        let after_us = u64::try_from(index).unwrap() * frame_us;
        // 0.4 s tolerated, then 2 s, each up to a frame late.
        assert!(
            (2_400_000..=2_400_000 + 2 * frame_us).contains(&after_us),
            "{frame_us} µs frames: adopted after {after_us} µs"
        );
    }
}

#[test]
fn the_game_profile_adopts_a_change_in_half_a_second_after_a_tenth() {
    let mut detector = BarDetector::new(auto(BlackBarMode::Default), Profile::Game);
    feed(&mut detector, SIZE, &letterbox_picture(), 0, FRAME, 2);
    let wide = boxed(W, H, 0, W, 8, 28);
    let found = feed(&mut detector, SIZE, &wide, 2 * FRAME, FRAME, 50);
    // Differing from frame 0 (80 ms); tolerated while 0.1 s or less, watched
    // from frame 3 (200 ms), adopted when it has been watched 0.5 s: at
    // frame 16 (720 ms).
    assert_eq!(first_at(&found, bars(9, 0)), Some(16));
    assert_eq!(found[15], bars(6, 0));
}

/// The game profile doesn't look at the outer 5%: a HUD in the first columns,
/// which ends the video profile's scan at once, is skipped.
#[test]
fn the_game_profile_ignores_a_hud_at_the_edges() {
    let mut pixels = boxed(W, H, 8, 56, 0, H);
    // A HUD bar in columns 0..3 across the middle rows, where the scan looks.
    light(&mut pixels, W, (0, 3), (8, 28));
    let mut video = video();
    assert_eq!(
        feed(&mut video, SIZE, &pixels, 0, FRAME, 1)[0],
        Bars::default()
    );
    let mut game = BarDetector::new(auto(BlackBarMode::Default), Profile::Game);
    assert_eq!(feed(&mut game, SIZE, &pixels, 0, FRAME, 1)[0], bars(0, 9));
}

#[test]
fn a_picture_that_starts_at_the_edge_has_no_bars_for_the_game_profile_either() {
    let pixels = boxed(W, H, 0, W, 0, H);
    let mut game = BarDetector::new(auto(BlackBarMode::Default), Profile::Game);
    assert_eq!(
        feed(&mut game, SIZE, &pixels, 0, FRAME, 1)[0],
        Bars::default()
    );
}

// The presets ---------------------------------------------------------------

#[test]
fn the_presets_override_what_is_detected_and_take_effect_at_once() {
    let plain = boxed(W, H, 0, W, 0, H);
    let film = letterbox_picture();

    let mut off = BarDetector::new(
        settings(BlackBarPreset::Off, BlackBarMode::Default),
        Profile::Video,
    );
    assert_eq!(off.update(W, H, &film, 0), Bars::default());

    // 21:9 in 64 × 36: the picture is 64 × 27 (586 / 21 rounded down), so 4 rows
    // on each side, with no blur margin, on a frame that shows none.
    let mut cinema = BarDetector::new(
        settings(BlackBarPreset::Cinema, BlackBarMode::Default),
        Profile::Video,
    );
    assert_eq!(cinema.update(W, H, &plain, 0), bars(4, 0));
    // 4:3 in 64 × 36: the picture is 48 × 36, 8 columns on each side.
    let mut classic = BarDetector::new(
        settings(BlackBarPreset::Classic, BlackBarMode::Default),
        Profile::Video,
    );
    assert_eq!(classic.update(W, H, &film, 0), bars(0, 8));
}

#[test]
fn a_preset_that_matches_the_frame_shape_has_no_bars() {
    let pixels = vec![9u8; 48 * 36 * 3];
    let mut classic = BarDetector::new(
        settings(BlackBarPreset::Classic, BlackBarMode::Default),
        Profile::Video,
    );
    assert_eq!(classic.update(48, 36, &pixels, 0), Bars::default());
}

#[test]
fn changing_to_auto_in_place_starts_detecting_and_changing_the_mode_forgets_the_old_border() {
    let film = letterbox_picture();
    let mut detector = BarDetector::new(
        settings(BlackBarPreset::Off, BlackBarMode::Default),
        Profile::Video,
    );
    assert_eq!(detector.update(W, H, &film, 0), Bars::default());
    detector.reconfigure(auto(BlackBarMode::Default), Profile::Video);
    assert_eq!(detector.update(W, H, &film, FRAME), bars(6, 0));
    // The same settings again keep what was seen; a new mode starts over.
    detector.reconfigure(auto(BlackBarMode::Default), Profile::Video);
    assert_eq!(
        detector.update(W, H, &boxed(W, H, 0, W, 8, 28), 2 * FRAME),
        bars(6, 0)
    );
    detector.reconfigure(auto(BlackBarMode::Letterbox), Profile::Video);
    assert_eq!(
        detector.update(W, H, &boxed(W, H, 0, W, 8, 28), 3 * FRAME),
        bars(9, 0)
    );
}

#[test]
fn a_frame_of_another_size_starts_over() {
    let mut detector = video();
    assert_eq!(detector.update(W, H, &letterbox_picture(), 0), bars(6, 0));
    let small = boxed(32, 18, 0, 32, 3, 15);
    assert_eq!(detector.update(32, 18, &small, FRAME), bars(4, 0));
}

#[test]
fn a_frame_with_the_wrong_length_changes_nothing() {
    let mut detector = video();
    assert_eq!(detector.update(W, H, &letterbox_picture(), 0), bars(6, 0));
    assert_eq!(detector.update(W, H, &[0; 10], FRAME), bars(6, 0));
}

/// Bars that would leave no row or column are cut to fit: at least one of
/// each remains, and they fit the frame's header (`Bars::fit`).
#[test]
fn the_bars_always_leave_a_row_and_a_column() {
    // 12 × 12, black but for the cells 3..9: found at 3, +1 for the blur.
    let pixels = boxed(12, 12, 3, 9, 3, 9);
    let mut detector = video();
    let found = detector.update(12, 12, &pixels, 0);
    assert!(found.fit(12, 12));
    assert_eq!(found, bars(4, 4));
}

// The pipeline ---------------------------------------------------------------

#[test]
fn bars_are_set_on_a_pictures_from_the_synthetic_backend_and_reported_as_fractions() {
    let mut picture = SyntheticBackend::render(Pattern::Letterbox, 0, W, H);
    let reported = picture.bars;
    assert_eq!(reported, bars(4, 0));
    picture.bars = Bars::default();
    let mut detector = video();
    detector.annotate(&mut picture, 0);
    // What the backend drew, 4 rows, and the blur margin.
    assert_eq!(picture.bars, bars(5, 0));
    let fractions = picture.bars.fractions(W, H);
    assert_eq!(
        fractions,
        BarFractions {
            top: 5.0 / 36.0,
            bottom: 5.0 / 36.0,
            left: 0.0,
            right: 0.0
        }
    );
}

#[test]
fn held_and_protected_pictures_carry_the_bars_in_force_and_are_not_looked_at() {
    let mut detector = video();
    let mut first = SyntheticBackend::render(Pattern::Letterbox, 0, W, H);
    detector.annotate(&mut first, 0);
    for flags in [
        FrameFlags {
            held: true,
            ..FrameFlags::default()
        },
        FrameFlags {
            protected: true,
            ..FrameFlags::default()
        },
    ] {
        // Pixels that would give other bars, if they were looked at.
        let mut other = SyntheticBackend::render(Pattern::ColorBars, 1, W, H);
        other.flags = flags;
        detector.annotate(&mut other, FRAME);
        assert_eq!(other.bars, first.bars);
    }
    assert_eq!(detector.bars(), first.bars);
}
