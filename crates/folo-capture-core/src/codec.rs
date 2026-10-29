//! The `FOLC` screen frame (docs/specs/source-link.md#screen-frames): a
//! 36-byte little-endian header and `width × height × 3` bytes of rgb8.
//! [`decode`] reads untrusted bytes: every field is checked before anything
//! is allocated or indexed.

use crate::error::FrameError;
use crate::types::{Bars, FrameFlags, MAX_HEIGHT, MAX_WIDTH, Picture, ScreenFrame};

/// The header's size.
pub const HEADER_LEN: usize = 36;
/// No message on the capture link may exceed this (ADR-0037 D2). A frame's
/// exact length is tighter (388,836 at most), so the type byte doesn't matter.
pub const MAX_MESSAGE_BYTES: usize = 400_000;
/// The message type byte of a screen frame on the links (`0x04`).
pub const MESSAGE_TYPE: u8 = 0x04;

const MAGIC: &[u8; 4] = b"FOLC";
const VERSION: u8 = 1;
const PIXEL_FORMAT_RGB8: u8 = 1;

/// The size of a frame message of this many cells.
#[must_use]
pub fn message_len(width: u16, height: u16) -> usize {
    HEADER_LEN + 3 * usize::from(width) * usize::from(height)
}

/// Writes a frame as header and payload (without the link's type byte).
///
/// # Errors
///
/// A [`FrameError`] when the frame breaks a rule the decoder would refuse:
/// the size, the bars, or pixels that aren't `width × height × 3` bytes.
pub fn encode(frame: &ScreenFrame) -> Result<Vec<u8>, FrameError> {
    let picture = &frame.picture;
    check_size(picture.width, picture.height)?;
    if !picture.bars.fit(picture.width, picture.height) {
        return Err(FrameError::BadBars);
    }
    let expected = message_len(picture.width, picture.height) - HEADER_LEN;
    if picture.pixels.len() != expected {
        return Err(FrameError::BadPixels {
            expected,
            actual: picture.pixels.len(),
        });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + expected);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[VERSION, PIXEL_FORMAT_RGB8, picture.flags.bits(), 0]);
    out.extend_from_slice(&picture.screen_index.to_le_bytes());
    out.extend_from_slice(&picture.width.to_le_bytes());
    out.extend_from_slice(&picture.height.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&frame.sequence.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_us.to_le_bytes());
    for bar in [
        picture.bars.top,
        picture.bars.bottom,
        picture.bars.left,
        picture.bars.right,
    ] {
        out.extend_from_slice(&bar.to_le_bytes());
    }
    out.extend_from_slice(&picture.pixels);
    Ok(out)
}

/// Reads a frame from a message (without the link's type byte). The reserved
/// bytes are not checked: a frame format version change is what changes them.
///
/// # Errors
///
/// The first [`FrameError`] found: the length, magic, version, pixel format,
/// flags, size, exact length, then the bars.
pub fn decode(message: &[u8]) -> Result<ScreenFrame, FrameError> {
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(FrameError::MessageTooLong {
            length: message.len(),
        });
    }
    if message.len() < HEADER_LEN {
        return Err(FrameError::TooShort {
            length: message.len(),
        });
    }
    if &message[0..4] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    if message[4] != VERSION {
        return Err(FrameError::BadVersion(message[4]));
    }
    if message[5] != PIXEL_FORMAT_RGB8 {
        return Err(FrameError::BadPixelFormat(message[5]));
    }
    let flags = FrameFlags::from_bits(message[6]).ok_or(FrameError::UnknownFlags(message[6]))?;
    let (width, height) = (u16_at(message, 10), u16_at(message, 12));
    check_size(width, height)?;
    let expected = message_len(width, height);
    if message.len() != expected {
        return Err(FrameError::BadLength {
            expected,
            actual: message.len(),
        });
    }
    let bars = Bars {
        top: u16_at(message, 28),
        bottom: u16_at(message, 30),
        left: u16_at(message, 32),
        right: u16_at(message, 34),
    };
    if !bars.fit(width, height) {
        return Err(FrameError::BadBars);
    }
    Ok(ScreenFrame {
        sequence: u32::from_le_bytes([message[16], message[17], message[18], message[19]]),
        timestamp_us: u64_at(message, 20),
        picture: Picture {
            screen_index: u16_at(message, 8),
            width,
            height,
            flags,
            bars,
            pixels: message[HEADER_LEN..].to_vec(),
        },
    })
}

fn check_size(width: u16, height: u16) -> Result<(), FrameError> {
    if !(1..=MAX_WIDTH).contains(&width) {
        return Err(FrameError::BadWidth(width));
    }
    if !(1..=MAX_HEIGHT).contains(&height) {
        return Err(FrameError::BadHeight(height));
    }
    Ok(())
}

/// The caller has checked that `at + 2` is inside the header.
fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut eight = [0; 8];
    eight.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(eight)
}
