//! From the colours of a frame to the bytes of a pixel run.
//!
//! The adapter chain has already put the colours in the target's colour model; this only picks
//! the bytes the device takes. Levels are truncated toward zero, as everywhere else. WLED's
//! realtime data has no colour-temperature channel, so a CCT element sends its brightness as
//! white and its kelvin is dropped; an on/off element is white at full or nothing.

use folo_provider::{ElementColors, ProviderError};

use crate::ddp::PixelFormat;
use crate::describe::WledTarget;

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn byte(level: f32) -> u8 {
    level as u8
}

/// One element as red, green, blue and white.
fn quad(colors: &ElementColors, index: usize) -> [u8; 4] {
    match colors {
        ElementColors::Rgb(v) => [byte(v[index][0]), byte(v[index][1]), byte(v[index][2]), 0],
        ElementColors::Rgbw(v) => v[index].map(byte),
        ElementColors::RgbCct(v) => {
            let e = &v[index];
            [
                byte(e.rgb[0]),
                byte(e.rgb[1]),
                byte(e.rgb[2]),
                byte(e.white),
            ]
        }
        ElementColors::Cct(v) => [0, 0, 0, byte(v[index].brightness)],
        ElementColors::Dimmer(v) => [0, 0, 0, byte(v[index])],
        ElementColors::OnOff(v) => [0, 0, 0, if v[index] { u8::MAX } else { 0 }],
        // Rejected before
        ElementColors::Xy(_) => [0; 4],
    }
}

/// Whether the element carries colour, or only white.
fn has_color(colors: &ElementColors) -> bool {
    matches!(
        colors,
        ElementColors::Rgb(_) | ElementColors::Rgbw(_) | ElementColors::RgbCct(_)
    )
}

/// The pixels of a frame for a target: the format and the bytes, at most as many pixels as the
/// target has. A target that takes white gets four bytes a pixel; otherwise three, and white-only
/// elements go out as grey, which every auto-white mode turns into white.
///
/// # Errors
/// [`ProviderError::Failed`] for xy colours, which WLED has no use for.
pub fn pixel_bytes(
    colors: &ElementColors,
    target: &WledTarget,
) -> Result<(PixelFormat, Vec<u8>), ProviderError> {
    if matches!(colors, ElementColors::Xy(_)) {
        return Err(ProviderError::Failed(format!(
            "{}: WLED takes no xy colours",
            target.id
        )));
    }
    let count = colors.len().min(target.count as usize);
    let format = if target.send_white {
        PixelFormat::Rgbw
    } else {
        PixelFormat::Rgb
    };
    let grey = !target.send_white && !has_color(colors);
    let mut out = Vec::with_capacity(count * format.bytes());
    for index in 0..count {
        let [r, g, b, w] = quad(colors, index);
        match (format, grey) {
            (PixelFormat::Rgbw, _) => out.extend([r, g, b, w]),
            (PixelFormat::Rgb, true) => out.extend([w, w, w]),
            (PixelFormat::Rgb, false) => out.extend([r, g, b]),
        }
    }
    Ok((format, out))
}
