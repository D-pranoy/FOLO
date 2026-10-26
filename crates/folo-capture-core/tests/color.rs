//! The colour and sizing reference: linear-light averaging of sRGB and the
//! fit-in-box rules of ADR-0037 D5, D6. The expected values were worked out
//! independently with the sRGB transfer functions of IEC 61966-2-1.

use folo_capture_core::color::{downscale, linear_to_srgb, srgb_to_linear};
use folo_capture_core::error::FrameError;
use folo_capture_core::sizing::{fit_in_box, requested_box};

fn grey(values: &[u8]) -> Vec<u8> {
    values.iter().flat_map(|&v| [v, v, v]).collect()
}

#[test]
fn the_transfer_functions_agree_at_known_points() {
    assert_eq!(srgb_to_linear(0), 0.0);
    assert!((srgb_to_linear(255) - 1.0).abs() < 1e-12);
    assert!((srgb_to_linear(128) - 0.215_860_500_113_899_26).abs() < 1e-12);
    // Below the knee of the curve: 1 / 255 / 12.92.
    assert!((srgb_to_linear(1) - 0.000_303_526_983_548_837_6).abs() < 1e-15);
}

#[test]
fn every_byte_survives_linear_and_back() {
    for value in 0..=255_u8 {
        assert_eq!(linear_to_srgb(srgb_to_linear(value)), value);
    }
}

#[test]
fn encoding_clamps_and_rounds_to_nearest() {
    assert_eq!(linear_to_srgb(-1.0), 0);
    assert_eq!(linear_to_srgb(2.0), 255);
    assert_eq!(linear_to_srgb(0.5), 188);
    assert_eq!(linear_to_srgb(f64::NAN), 0);
}

#[test]
fn black_and_white_average_to_188_not_128() {
    assert_eq!(
        downscale(&grey(&[0, 255]), 2, 1, 1, 1).unwrap(),
        grey(&[188])
    );
}

#[test]
fn a_two_by_two_with_one_white_cell_is_137() {
    assert_eq!(
        downscale(&grey(&[0, 255, 0, 0]), 2, 2, 1, 1).unwrap(),
        grey(&[137])
    );
}

#[test]
fn cells_are_weighted_by_how_much_they_cover() {
    // Three to one: the middle cell (black) is split 2/3 and 1/3 between the two outputs.
    assert_eq!(
        downscale(&grey(&[255, 255, 0]), 3, 1, 1, 1).unwrap(),
        grey(&[213])
    );
    assert_eq!(
        downscale(&grey(&[255, 0, 255]), 3, 1, 2, 1).unwrap(),
        grey(&[213, 213])
    );
    assert_eq!(
        downscale(&grey(&[0, 255, 0]), 3, 1, 2, 1).unwrap(),
        grey(&[156, 156])
    );
}

#[test]
fn the_same_size_and_flat_colours_come_back_unchanged() {
    let picture: Vec<u8> = (0..4_u32 * 3 * 3)
        .map(|i| u8::try_from(i * 7 % 256).unwrap())
        .collect();
    assert_eq!(downscale(&picture, 4, 3, 4, 3).unwrap(), picture);
    let flat = [200, 30, 90].repeat(16 * 9);
    assert_eq!(
        downscale(&flat, 16, 9, 4, 3).unwrap(),
        [200, 30, 90].repeat(12)
    );
}

#[test]
fn channels_are_averaged_on_their_own_and_rows_after_columns() {
    // Red/blue chequer 2x2: every output channel is the mean of its own channel.
    let chequer = [255, 0, 0, 0, 0, 255, 0, 0, 255, 255, 0, 0];
    assert_eq!(downscale(&chequer, 2, 2, 1, 1).unwrap(), vec![188, 0, 188]);
}

#[test]
fn a_picture_scaled_up_repeats_its_cells() {
    assert_eq!(
        downscale(&grey(&[10, 200]), 2, 1, 4, 1).unwrap(),
        grey(&[10, 10, 200, 200])
    );
}

#[test]
fn bad_sizes_and_lengths_are_errors() {
    assert_eq!(downscale(&[], 0, 1, 1, 1), Err(FrameError::BadWidth(0)));
    assert_eq!(
        downscale(&grey(&[0]), 1, 1, 0, 1),
        Err(FrameError::BadWidth(0))
    );
    assert_eq!(
        downscale(&grey(&[0]), 1, 0, 1, 1),
        Err(FrameError::BadHeight(0))
    );
    assert_eq!(
        downscale(&grey(&[0]), 1, 1, 1, 0),
        Err(FrameError::BadHeight(0))
    );
    assert_eq!(
        downscale(&[0; 5], 1, 2, 1, 1),
        Err(FrameError::BadPixels {
            expected: 6,
            actual: 5
        })
    );
}

#[test]
fn a_full_screen_scales_to_the_largest_frame() {
    assert_eq!(fit_in_box(1920, 1080, 480, 270), (480, 270));
    assert_eq!(fit_in_box(3840, 2160, 480, 270), (480, 270));
    assert_eq!(fit_in_box(1920, 1080, 128, 72), (128, 72));
}

#[test]
fn other_aspect_ratios_fit_the_box_rounding_down() {
    // 21:9 is width limited: 480 * 1440 / 3440 = 200.9.
    assert_eq!(fit_in_box(3440, 1440, 480, 270), (480, 200));
    // 4:3 is height limited: 270 * 4 / 3 = 360.
    assert_eq!(fit_in_box(1600, 1200, 480, 270), (360, 270));
    // A portrait screen.
    assert_eq!(fit_in_box(1080, 1920, 480, 270), (151, 270));
    assert_eq!(fit_in_box(1920, 1080, 64, 36), (64, 36));
}

#[test]
fn a_frame_is_never_larger_than_the_screen_and_never_empty() {
    assert_eq!(fit_in_box(320, 200, 480, 270), (320, 200));
    assert_eq!(fit_in_box(100, 5000, 480, 270), (5, 270));
    assert_eq!(fit_in_box(5000, 1, 480, 270), (480, 1));
}

#[test]
fn an_unknown_screen_size_gets_the_box() {
    assert_eq!(fit_in_box(0, 0, 480, 270), (480, 270));
    assert_eq!(fit_in_box(1920, 0, 96, 54), (96, 54));
}

#[test]
fn the_fit_stays_inside_the_box_for_a_sweep_of_screens() {
    for (width, height) in [
        (1, 1),
        (7, 3),
        (800, 600),
        (1366, 768),
        (2560, 1080),
        (5120, 1440),
        (7680, 4320),
    ] {
        for (box_width, box_height) in [(64, 36), (100, 100), (480, 270)] {
            let (w, h) = fit_in_box(width, height, box_width, box_height);
            assert!(
                w >= 1 && h >= 1 && w <= box_width && h <= box_height,
                "{width}x{height} in {box_width}x{box_height}: {w}x{h}"
            );
        }
    }
}

#[test]
fn the_box_the_node_asks_for_is_twice_the_canvas_within_its_limits() {
    assert_eq!(requested_box((64, 36)), (128, 72));
    assert_eq!(requested_box((16, 9)), (64, 36));
    assert_eq!(requested_box((128, 72)), (256, 144));
    assert_eq!(requested_box((400, 400)), (480, 270));
    assert_eq!(requested_box((u16::MAX, u16::MAX)), (480, 270));
}
