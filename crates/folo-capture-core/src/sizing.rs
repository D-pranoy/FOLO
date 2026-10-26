//! The size of the frames (ADR-0037 D5): the largest of the screen's aspect
//! ratio that fits the box the node asked for.

use crate::types::{MAX_HEIGHT, MAX_WIDTH};

/// The box the node asks for: twice the largest canvas, at least 64 × 36 and
/// at most 480 × 270 (D5).
#[must_use]
pub fn requested_box(largest_canvas: (u16, u16)) -> (u16, u16) {
    let double = |cells: u16, least: u16, most: u16| cells.saturating_mul(2).clamp(least, most);
    (
        double(largest_canvas.0, 64, MAX_WIDTH),
        double(largest_canvas.1, 36, MAX_HEIGHT),
    )
}

/// The largest frame of a `screen_width` × `screen_height` screen's aspect
/// ratio that fits `box_width` × `box_height`, rounded down so it fits, at
/// least 1 × 1. It is never larger than the screen itself.
///
/// A screen of an unknown size (0) gets the box.
#[must_use]
pub fn fit_in_box(
    screen_width: u32,
    screen_height: u32,
    box_width: u16,
    box_height: u16,
) -> (u16, u16) {
    if screen_width == 0 || screen_height == 0 {
        return (box_width.max(1), box_height.max(1));
    }
    let (sw, sh) = (u64::from(screen_width), u64::from(screen_height));
    let (bw, bh) = (u64::from(box_width), u64::from(box_height));
    // Compare the aspect ratios by cross-multiplying, to stay in integers.
    let (mut width, mut height) = if bw * sh <= bh * sw {
        (bw, bw * sh / sw)
    } else {
        (bh * sw / sh, bh)
    };
    if width > sw || height > sh {
        (width, height) = (sw, sh);
    }
    // Both are at most 480 × 270 here, so they fit a u16.
    (
        u16::try_from(width.max(1)).unwrap_or(MAX_WIDTH),
        u16::try_from(height.max(1)).unwrap_or(MAX_HEIGHT),
    )
}
