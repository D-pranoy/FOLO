//! The colour reference (ADR-0037 D6): sRGB is averaged in linear light.
//! [`downscale`] is what the backends' shaders and the CPU path are tested
//! against: each output cell is the area-weighted mean, in linear light, of
//! the input cells under it, encoded back to sRGB and rounded to nearest.

use std::sync::OnceLock;

use crate::error::FrameError;

/// The linear light (0 to 1) of an sRGB-encoded byte.
#[must_use]
pub fn srgb_to_linear(value: u8) -> f64 {
    static TABLE: OnceLock<[f64; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0.0; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let encoded = f64::from(u8::try_from(index).unwrap_or(u8::MAX)) / 255.0;
            *slot = if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            };
        }
        table
    });
    table[usize::from(value)]
}

/// The sRGB-encoded byte (rounded to nearest) of a linear light, clamped to 0 to 1.
#[must_use]
pub fn linear_to_srgb(linear: f64) -> u8 {
    let linear = linear.clamp(0.0, 1.0);
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    // The value is in 0 to 255 after rounding, so the cast can't truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = (encoded * 255.0).round() as u8;
    byte
}

/// For each output cell along one axis, the input cells under it and how much
/// of each, as shares of the output cell's span.
fn weights(input: u32, output: u32) -> Vec<Vec<(usize, f64)>> {
    let scale = f64::from(input) / f64::from(output);
    (0..output)
        .map(|cell| {
            let (from, to) = (f64::from(cell) * scale, f64::from(cell + 1) * scale);
            // The cast is of a non-negative value, and `input` bounds it.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (first, last) = (from.floor() as u32, (to.ceil() as u32).min(input));
            (first..last)
                .map(|source| {
                    let overlap = to.min(f64::from(source + 1)) - from.max(f64::from(source));
                    (source as usize, overlap / scale)
                })
                .filter(|(_, share)| *share > 0.0)
                .collect()
        })
        .collect()
}

/// Downscales `pixels` (`in_width × in_height` sRGB rgb8, row-major) to
/// `out_width × out_height`.
///
/// # Errors
///
/// [`FrameError::BadWidth`] or [`FrameError::BadHeight`] for a size of 0, and
/// [`FrameError::BadPixels`] when `pixels` isn't `in_width × in_height × 3` bytes.
pub fn downscale(
    pixels: &[u8],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Result<Vec<u8>, FrameError> {
    let to_u16 = |cells: usize| u16::try_from(cells).unwrap_or(u16::MAX);
    if in_width == 0 || out_width == 0 {
        return Err(FrameError::BadWidth(to_u16(in_width.min(out_width))));
    }
    if in_height == 0 || out_height == 0 {
        return Err(FrameError::BadHeight(to_u16(in_height.min(out_height))));
    }
    // Sizes are at most a screen's: far below 2^32.
    let to_u32 = |cells: usize| u32::try_from(cells).map_err(|_| FrameError::BadWidth(u16::MAX));
    let expected = in_width * in_height * 3;
    if pixels.len() != expected {
        return Err(FrameError::BadPixels {
            expected,
            actual: pixels.len(),
        });
    }
    let (across, down) = (
        weights(to_u32(in_width)?, to_u32(out_width)?),
        weights(to_u32(in_height)?, to_u32(out_height)?),
    );
    // Across first, per input row, in linear light.
    let mut rows = vec![0.0_f64; in_height * out_width * 3];
    for row in 0..in_height {
        for (x, parts) in across.iter().enumerate() {
            for channel in 0..3 {
                rows[(row * out_width + x) * 3 + channel] = parts
                    .iter()
                    .map(|&(source, share)| {
                        srgb_to_linear(pixels[(row * in_width + source) * 3 + channel]) * share
                    })
                    .sum();
            }
        }
    }
    let mut out = Vec::with_capacity(out_width * out_height * 3);
    for parts in &down {
        for x in 0..out_width {
            for channel in 0..3 {
                let linear: f64 = parts
                    .iter()
                    .map(|&(row, share)| rows[(row * out_width + x) * 3 + channel] * share)
                    .sum();
                out.push(linear_to_srgb(linear));
            }
        }
    }
    Ok(out)
}
