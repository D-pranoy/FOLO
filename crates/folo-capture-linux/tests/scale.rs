//! The CPU downscale against the core's reference (ADR-0037 D17).
#![cfg(target_os = "linux")]

use folo_capture_core::color::downscale;
use folo_capture_linux::scale::{Downscaler, PixelLayout, SourceImage};

/// A deterministic picture: `(r, g, b)` of each pixel from its position.
fn pattern(width: usize, height: usize) -> Vec<[u8; 3]> {
    (0..width * height)
        .map(|index| {
            let (x, y) = (index % width, index / width);
            // The products stay below 2^16, and `% 256` fits a byte.
            let byte = |value: usize| u8::try_from(value % 256).unwrap();
            [byte(x * 7 + y), byte(y * 11 + x * 3), byte(x * y + 40)]
        })
        .collect()
}

/// The same picture laid out as `layout` with `stride`, padding zeroed.
fn laid_out(pixels: &[[u8; 3]], width: usize, layout: PixelLayout, stride: usize) -> Vec<u8> {
    let bytes = layout.bytes_per_pixel();
    let height = pixels.len() / width;
    let mut data = vec![0xEE; stride * height];
    for (index, [r, g, b]) in pixels.iter().copied().enumerate() {
        let (x, y) = (index % width, index / width);
        let at = y * stride + x * bytes;
        let rgb = match layout {
            PixelLayout::Bgrx | PixelLayout::Bgra | PixelLayout::Bgr => [b, g, r],
            PixelLayout::Rgbx | PixelLayout::Rgba | PixelLayout::Rgb => [r, g, b],
        };
        data[at..at + 3].copy_from_slice(&rgb);
        if bytes == 4 {
            // The fourth byte is junk or alpha either way.
            data[at + 3] = 0x13;
        }
    }
    data
}

fn flat(pixels: &[[u8; 3]]) -> Vec<u8> {
    pixels.iter().flatten().copied().collect()
}

#[test]
fn every_layout_gives_the_references_picture_within_a_level() {
    let (width, height, out_width, out_height) = (53, 31, 17, 9);
    let pixels = pattern(width, height);
    let reference = downscale(&flat(&pixels), width, height, out_width, out_height).unwrap();
    for layout in [
        PixelLayout::Bgrx,
        PixelLayout::Bgra,
        PixelLayout::Rgbx,
        PixelLayout::Rgba,
        PixelLayout::Rgb,
        PixelLayout::Bgr,
    ] {
        // A stride with padding: the rows are not tightly packed.
        let stride = width * layout.bytes_per_pixel() + 12;
        let data = laid_out(&pixels, width, layout, stride);
        let image = SourceImage {
            layout,
            width,
            height,
            stride,
            data: &data,
        };
        let mut scaler = Downscaler::new(width, height, out_width, out_height).unwrap();
        let picture = scaler.scale(&image).unwrap();
        assert_eq!(picture.len(), reference.len(), "{layout:?}");
        let worst = picture
            .iter()
            .zip(&reference)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "{layout:?}: off by {worst}");
    }
}

#[test]
fn a_flat_colour_stays_exact_and_the_scaler_is_reusable() {
    let (width, height) = (64, 36);
    let pixels = vec![[200, 30, 120]; width * height];
    let data = laid_out(&pixels, width, PixelLayout::Bgrx, width * 4);
    let image = SourceImage {
        layout: PixelLayout::Bgrx,
        width,
        height,
        stride: width * 4,
        data: &data,
    };
    let mut scaler = Downscaler::new(width, height, 16, 9).unwrap();
    for _ in 0..3 {
        let picture = scaler.scale(&image).unwrap();
        assert!(picture.chunks(3).all(|cell| cell == [200, 30, 120]));
    }
}

#[test]
fn averaging_is_in_linear_light() {
    // Half white, half black averages to sRGB 188, not 128.
    let data = [255u8, 255, 255, 0, 0, 0];
    let image = SourceImage {
        layout: PixelLayout::Rgb,
        width: 2,
        height: 1,
        stride: 6,
        data: &data,
    };
    let picture = Downscaler::new(2, 1, 1, 1).unwrap().scale(&image).unwrap();
    assert_eq!(picture, [188, 188, 188]);
}

#[test]
fn a_buffer_that_does_not_fit_is_refused() {
    let full = vec![0u8; 4 * 4 * 4];
    let accepted = |width: usize, stride: usize, data: &[u8]| {
        let image = SourceImage {
            layout: PixelLayout::Bgrx,
            width,
            height: 4,
            stride,
            data,
        };
        Downscaler::new(4, 4, 2, 2).unwrap().scale(&image).is_ok()
    };
    assert!(accepted(4, 16, &full));
    assert!(!accepted(5, 20, &[0u8; 80]), "wrong width");
    assert!(!accepted(4, 12, &full), "stride under a row");
    assert!(!accepted(4, 16, &full[..20]), "data under the rows");
    assert!(Downscaler::new(0, 4, 2, 2).is_err());
    assert!(Downscaler::new(4, 4, 2, 0).is_err());
}

#[test]
fn the_downscaler_knows_its_sizes() {
    let scaler = Downscaler::new(8, 6, 4, 3).unwrap();
    assert!(scaler.is_for((8, 6), (4, 3)));
    assert!(!scaler.is_for((8, 6), (4, 2)));
    assert!(!scaler.is_for((9, 6), (4, 3)));
}

#[test]
fn every_size_pair_the_agent_meets_matches_the_reference() {
    let sizes = [
        (640, 360, 64, 36),
        (640, 360, 100, 56),
        (640, 360, 32, 18),
        (1920, 1080, 480, 270),
        (1921, 1081, 480, 270),
        (100, 50, 480, 240),
        (7, 5, 3, 2),
        (480, 270, 480, 270),
        (3, 3, 1, 1),
    ];
    for (in_w, in_h, out_w, out_h) in sizes {
        let pixels = pattern(in_w, in_h);
        let reference = downscale(&flat(&pixels), in_w, in_h, out_w, out_h).unwrap();
        let data = laid_out(&pixels, in_w, PixelLayout::Bgrx, in_w * 4 + 8);
        let image = SourceImage {
            layout: PixelLayout::Bgrx,
            width: in_w,
            height: in_h,
            stride: in_w * 4 + 8,
            data: &data,
        };
        let picture = Downscaler::new(in_w, in_h, out_w, out_h)
            .unwrap()
            .scale(&image)
            .unwrap();
        let worst = picture
            .iter()
            .zip(&reference)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(
            worst <= 1,
            "{in_w}x{in_h} to {out_w}x{out_h}: off by {worst}"
        );
    }
}
