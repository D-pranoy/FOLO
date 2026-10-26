//! The downscale and tone-mapping shader on the WARP software device
//! (ADR-0037 D22), against the colour reference of `folo-capture-core` and the
//! tone-map reference of this crate. Synthetic textures only: nothing here
//! captures a screen, and WARP needs no GPU, so CI's Windows job runs these.
//!
//! The tolerance is one level of 8-bit output per channel: the shader works
//! in 32-bit floats and the reference in 64-bit ones, and both round to nearest.
#![cfg(windows)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use folo_capture_core::color;
use folo_capture_windows::gpu::{BGRA8, Downscaler, FP16, Gpu, shader_resource_binding};
use folo_capture_windows::tonemap;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};

const TOLERANCE: u8 = 1;

fn downscaler() -> Downscaler {
    Downscaler::new(Gpu::warp().expect("the WARP software device")).expect("the shaders compile")
}

fn texture(
    gpu: &Gpu,
    format: DXGI_FORMAT,
    size: (u32, u32),
    bytes: &[u8],
    pixel_bytes: u32,
) -> ID3D11Texture2D {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.0,
        Height: size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: shader_resource_binding(),
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    gpu.texture_from(&desc, bytes, size.0 * pixel_bytes)
        .expect("a texture from bytes")
}

/// An 8-bit desktop texture (BGRA, as Desktop Duplication delivers it) of `rgb` pixels.
fn bgra8(gpu: &Gpu, size: (u32, u32), rgb: &[u8]) -> ID3D11Texture2D {
    let bytes: Vec<u8> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255])
        .collect();
    texture(gpu, BGRA8, size, &bytes, 4)
}

/// The bits of an IEEE half float, for values that are exact in it (normal, no rounding needed).
fn half(value: f32) -> u16 {
    if value == 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff).cast_signed() - 127 + 15;
    assert!(
        (1..31).contains(&exponent),
        "{value} isn't a normal half float"
    );
    assert_eq!(bits & 0x1fff, 0, "{value} isn't exact in a half float");
    sign | ((exponent as u16) << 10) | ((bits & 0x7f_ffff) >> 13) as u16
}

/// An scRGB FP16 desktop texture of `scrgb` pixels (rgb floats).
fn fp16(gpu: &Gpu, size: (u32, u32), scrgb: &[f32]) -> ID3D11Texture2D {
    let bytes: Vec<u8> = scrgb
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 1.0])
        .flat_map(|component| half(component).to_le_bytes())
        .collect();
    texture(gpu, FP16, size, &bytes, 8)
}

/// A deterministic picture with detail at every scale.
fn noise(width: usize, height: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    (0..width * height * 3)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn assert_close(actual: &[u8], expected: &[u8], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    let worst = actual
        .iter()
        .zip(expected)
        .map(|(a, e)| a.abs_diff(*e))
        .max()
        .unwrap_or(0);
    assert!(worst <= TOLERANCE, "{what}: off by {worst} levels");
}

#[test]
fn an_sdr_desktop_matches_the_core_reference_at_awkward_ratios() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    for (input, output) in [
        ((37, 23), (5, 3)),
        ((37, 23), (37, 23)),
        ((101, 59), (7, 4)),
        ((64, 36), (1, 1)),
        ((10, 6), (30, 18)),
        ((1920, 1080), (480, 270)),
        ((1366, 768), (480, 270)),
    ] {
        let pixels = noise(input.0, input.1);
        let source = bgra8(&gpu, (input.0 as u32, input.1 as u32), &pixels);
        let actual = scaler
            .downscale(&source, (output.0 as u16, output.1 as u16), None)
            .expect("downscale");
        let expected =
            color::downscale(&pixels, input.0, input.1, output.0, output.1).expect("reference");
        assert_close(&actual, &expected, &format!("{input:?} to {output:?}"));
    }
}

#[test]
fn channels_keep_their_order() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    // Red, green and blue pixels side by side.
    let source = bgra8(&gpu, (3, 1), &[255, 0, 0, 0, 255, 0, 0, 0, 255]);
    let pixels = scaler.downscale(&source, (3, 1), None).unwrap();
    assert_close(&pixels, &[255, 0, 0, 0, 255, 0, 0, 0, 255], "RGB order");
}

#[test]
fn averaging_is_in_linear_light() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    // Black and white: the mean of the light is 0.5, which sRGB encodes as 188, not 128.
    let source = bgra8(&gpu, (2, 1), &[0, 0, 0, 255, 255, 255]);
    let pixels = scaler.downscale(&source, (1, 1), None).unwrap();
    assert_close(&pixels, &[188, 188, 188], "black and white");
    assert_eq!(color::linear_to_srgb(0.5), 188);
}

#[test]
fn the_output_size_follows_each_call() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    let source = bgra8(&gpu, (16, 9), &noise(16, 9));
    for size in [(8_u16, 4_u16), (4, 2), (16, 9), (8, 4)] {
        let pixels = scaler.downscale(&source, size, None).unwrap();
        assert_eq!(pixels.len(), usize::from(size.0) * usize::from(size.1) * 3);
    }
}

/// The reference for an HDR picture: the mean of the scRGB light, tone-mapped, encoded.
fn hdr_reference(scrgb: &[f32], sdr_white_nits: f32) -> Vec<u8> {
    let mean: Vec<f32> = (0..3)
        .map(|channel| {
            let pixels = scrgb.len() / 3;
            scrgb.iter().skip(channel).step_by(3).sum::<f32>() / pixels as f32
        })
        .collect();
    let mapped = tonemap::tone_map(
        [mean[0], mean[1], mean[2]],
        tonemap::scale_for(sdr_white_nits),
    );
    mapped
        .map(|light| color::linear_to_srgb(f64::from(light)))
        .to_vec()
}

#[test]
fn an_hdr_desktop_is_tone_mapped_against_the_sdr_white_level() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    for (white_nits, colour) in [
        (80.0, [0.5_f32, 0.25, 0.125]),
        (80.0, [1.0, 1.0, 1.0]),
        (240.0, [3.0, 3.0, 3.0]),
        (240.0, [1.5, 0.75, 0.5]),
        (480.0, [2.0, 4.0, 1.0]),
        (240.0, [8.0, 4.0, 2.0]),
        (80.0, [0.0, 0.0, 0.0]),
    ] {
        let scrgb = colour.repeat(4);
        let source = fp16(&gpu, (2, 2), &scrgb);
        let scale = tonemap::scale_for(white_nits);
        let actual = scaler.downscale(&source, (1, 1), Some(scale)).unwrap();
        assert_close(
            &actual,
            &hdr_reference(&colour, white_nits),
            &format!("{colour:?} at {white_nits} nits"),
        );
    }
}

#[test]
fn sdr_white_is_white_and_a_highlight_keeps_its_hue() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    let white = fp16(&gpu, (1, 1), &[3.0, 3.0, 3.0]);
    let pixels = scaler
        .downscale(&white, (1, 1), Some(tonemap::scale_for(240.0)))
        .unwrap();
    assert_close(&pixels, &[255, 255, 255], "SDR white");
    // (8, 4, 2) is brighter than white: divided by its largest component, (1, 0.5, 0.25).
    let highlight = fp16(&gpu, (1, 1), &[8.0, 4.0, 2.0]);
    let pixels = scaler.downscale(&highlight, (1, 1), Some(1.0)).unwrap();
    assert_close(&pixels, &[255, 188, 137], "highlight");
}

#[test]
fn hdr_light_is_averaged_before_it_is_tone_mapped() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    // Black and a highlight of 4 (320 nits) at 80 nits white: the mean is 2, which clips to
    // white. Tone-mapping each pixel first would give black and white, whose mean is 188.
    let source = fp16(&gpu, (2, 1), &[0.0, 0.0, 0.0, 4.0, 4.0, 4.0]);
    let pixels = scaler.downscale(&source, (1, 1), Some(1.0)).unwrap();
    assert_close(&pixels, &[255, 255, 255], "average first");
}

#[test]
fn an_hdr_desktop_downscales_at_awkward_ratios() {
    let mut scaler = downscaler();
    let gpu = Gpu::warp().unwrap();
    // A ramp of exactly representable values, 0 to 3 in steps of 1/8, repeated across the picture.
    let (width, height) = (24_usize, 15_usize);
    let scrgb: Vec<f32> = (0..width * height * 3)
        .map(|index| (index % 25) as f32 / 8.0)
        .collect();
    let source = fp16(&gpu, (width as u32, height as u32), &scrgb);
    let scale = tonemap::scale_for(160.0);
    let actual = scaler.downscale(&source, (3, 3), Some(scale)).unwrap();
    // Each output pixel averages an 8 x 5 block. The reference computes each one by hand.
    let mut expected = Vec::new();
    for out_y in 0..3 {
        for out_x in 0..3 {
            let mut sum = [0.0_f32; 3];
            for y in out_y * 5..(out_y + 1) * 5 {
                for x in out_x * 8..(out_x + 1) * 8 {
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += scrgb[(y * width + x) * 3 + channel];
                    }
                }
            }
            let mean = sum.map(|total| total / 40.0);
            expected.extend(
                tonemap::tone_map(mean, scale).map(|light| color::linear_to_srgb(f64::from(light))),
            );
        }
    }
    assert_close(&actual, &expected, "an HDR ramp");
}
