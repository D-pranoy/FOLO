//! The FOLC frame codec against specs/fixtures/screenFrame--rgb8.folc, and
//! against every bound of docs/specs/source-link.md#screen-frames.

use folo_capture_core::codec::{HEADER_LEN, MAX_MESSAGE_BYTES, decode, encode, message_len};
use folo_capture_core::error::FrameError;
use folo_capture_core::types::{Bars, FrameFlags, Picture, ScreenFrame};

fn fixture() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specs/fixtures/screenFrame--rgb8.folc"
    );
    std::fs::read(path).unwrap()
}

fn frame(width: u16, height: u16) -> ScreenFrame {
    let pixels = (0..usize::from(width) * usize::from(height) * 3)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    ScreenFrame {
        sequence: 7,
        timestamp_us: 123_456_789,
        picture: Picture {
            screen_index: 2,
            width,
            height,
            flags: FrameFlags {
                protected: true,
                hdr: false,
                held: true,
            },
            bars: Bars {
                top: height.saturating_sub(1).min(1),
                bottom: 0,
                left: width.saturating_sub(1).min(2),
                right: 0,
            },
            pixels,
        },
    }
}

/// The fixture's header with one byte changed, and its payload cut or kept to fit.
fn with_byte(at: usize, value: u8) -> Vec<u8> {
    let mut bytes = fixture();
    bytes[at] = value;
    bytes
}

#[test]
fn the_fixture_decodes_to_its_documented_fields() {
    let decoded = decode(&fixture()).unwrap();
    assert_eq!(decoded.sequence, 42);
    assert_eq!(decoded.timestamp_us, 1_234_567);
    let picture = decoded.picture;
    assert_eq!(
        (picture.screen_index, picture.width, picture.height),
        (0, 8, 6)
    );
    assert_eq!(picture.flags, FrameFlags::default());
    assert_eq!(
        picture.bars,
        Bars {
            top: 1,
            bottom: 1,
            left: 0,
            right: 0
        }
    );
    assert_eq!(picture.pixels.len(), 8 * 6 * 3);
}

#[test]
fn encoding_the_fixture_back_gives_its_bytes() {
    assert_eq!(encode(&decode(&fixture()).unwrap()).unwrap(), fixture());
}

#[test]
fn a_frame_survives_a_round_trip_at_the_edges_of_the_sizes() {
    for (width, height) in [(1, 1), (1, 270), (480, 1), (64, 36), (480, 270)] {
        let original = frame(width, height);
        let bytes = encode(&original).unwrap();
        assert_eq!(bytes.len(), message_len(width, height));
        assert_eq!(decode(&bytes).unwrap(), original, "{width}x{height}");
    }
}

#[test]
fn the_largest_frame_is_388_836_bytes_and_under_the_message_cap() {
    assert_eq!(message_len(480, 270), 388_836);
    assert!(message_len(480, 270) <= MAX_MESSAGE_BYTES);
}

#[test]
fn every_flag_combination_round_trips() {
    for bits in 0..8_u8 {
        let mut original = frame(4, 4);
        original.picture.flags = FrameFlags::from_bits(bits).unwrap();
        original.picture.bars = Bars::default();
        let bytes = encode(&original).unwrap();
        assert_eq!(bytes[6], bits);
        assert_eq!(decode(&bytes).unwrap(), original);
    }
}

#[test]
fn a_message_over_the_cap_is_refused_before_anything_else() {
    let long = vec![0; MAX_MESSAGE_BYTES + 1];
    assert_eq!(
        decode(&long),
        Err(FrameError::MessageTooLong {
            length: MAX_MESSAGE_BYTES + 1
        })
    );
}

#[test]
fn every_truncation_of_the_fixture_is_refused() {
    let bytes = fixture();
    for length in 0..bytes.len() {
        assert!(decode(&bytes[..length]).is_err(), "{length} bytes");
    }
    assert_eq!(
        decode(&bytes[..HEADER_LEN - 1]),
        Err(FrameError::TooShort {
            length: HEADER_LEN - 1
        })
    );
}

#[test]
fn a_message_with_extra_bytes_is_refused() {
    let mut bytes = fixture();
    bytes.push(0);
    assert_eq!(
        decode(&bytes),
        Err(FrameError::BadLength {
            expected: 180,
            actual: 181
        })
    );
}

#[test]
fn the_magic_version_and_pixel_format_are_checked() {
    assert_eq!(decode(&with_byte(0, b'X')), Err(FrameError::BadMagic));
    assert_eq!(decode(&with_byte(3, b'S')), Err(FrameError::BadMagic));
    assert_eq!(decode(&with_byte(4, 0)), Err(FrameError::BadVersion(0)));
    assert_eq!(decode(&with_byte(4, 2)), Err(FrameError::BadVersion(2)));
    assert_eq!(decode(&with_byte(5, 0)), Err(FrameError::BadPixelFormat(0)));
    assert_eq!(decode(&with_byte(5, 2)), Err(FrameError::BadPixelFormat(2)));
}

#[test]
fn flag_bits_beyond_the_three_are_refused() {
    for bit in 3..8 {
        assert_eq!(
            decode(&with_byte(6, 1 << bit)),
            Err(FrameError::UnknownFlags(1 << bit))
        );
    }
    assert!(decode(&with_byte(6, 0b111)).is_ok());
}

#[test]
fn the_reserved_bytes_are_not_checked_and_are_written_as_zero() {
    let bytes = fixture();
    for at in [7, 14, 15] {
        assert!(decode(&with_byte(at, 0xFF)).is_ok(), "byte {at}");
        assert_eq!(bytes[at], 0);
    }
}

#[test]
fn width_and_height_must_be_inside_their_limits() {
    let set = |at: usize, value: u16| {
        let mut bytes = fixture();
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        decode(&bytes)
    };
    assert_eq!(set(10, 0), Err(FrameError::BadWidth(0)));
    assert_eq!(set(10, 481), Err(FrameError::BadWidth(481)));
    assert_eq!(set(10, u16::MAX), Err(FrameError::BadWidth(u16::MAX)));
    assert_eq!(set(12, 0), Err(FrameError::BadHeight(0)));
    assert_eq!(set(12, 271), Err(FrameError::BadHeight(271)));
    assert_eq!(set(12, u16::MAX), Err(FrameError::BadHeight(u16::MAX)));
    // Inside the limits, but not the size of the payload.
    assert_eq!(
        set(10, 9),
        Err(FrameError::BadLength {
            expected: 36 + 9 * 6 * 3,
            actual: 180
        })
    );
}

#[test]
fn the_bars_must_leave_a_row_and_a_column() {
    let set = |top: u16, bottom: u16, left: u16, right: u16| {
        let mut bytes = fixture();
        for (at, value) in [(28, top), (30, bottom), (32, left), (34, right)] {
            bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        decode(&bytes)
    };
    // The fixture is 8 wide and 6 high.
    assert!(set(3, 2, 4, 3).is_ok());
    assert_eq!(set(3, 3, 0, 0), Err(FrameError::BadBars));
    assert_eq!(set(6, 0, 0, 0), Err(FrameError::BadBars));
    assert_eq!(set(0, 0, 4, 4), Err(FrameError::BadBars));
    assert_eq!(set(0, 0, 8, 0), Err(FrameError::BadBars));
    assert_eq!(set(u16::MAX, u16::MAX, 0, 0), Err(FrameError::BadBars));
    assert_eq!(set(0, 0, u16::MAX, u16::MAX), Err(FrameError::BadBars));
}

#[test]
fn encoding_refuses_what_decoding_would() {
    assert_eq!(encode(&frame(0, 4)), Err(FrameError::BadWidth(0)));
    assert_eq!(encode(&frame(481, 4)), Err(FrameError::BadWidth(481)));
    assert_eq!(encode(&frame(4, 271)), Err(FrameError::BadHeight(271)));
    let mut bars = frame(4, 4);
    bars.picture.bars.top = 4;
    assert_eq!(encode(&bars), Err(FrameError::BadBars));
    let mut short = frame(4, 4);
    short.picture.pixels.pop();
    assert_eq!(
        encode(&short),
        Err(FrameError::BadPixels {
            expected: 48,
            actual: 47
        })
    );
}

#[test]
fn no_byte_of_the_header_changed_to_any_value_makes_decoding_panic() {
    for at in 0..HEADER_LEN {
        for value in 0..=255_u8 {
            let _ = decode(&with_byte(at, value));
        }
    }
}

#[test]
fn error_messages_name_the_field_and_hold_no_pixels() {
    let message = decode(&with_byte(10, 0)).unwrap_err().to_string();
    assert_eq!(message, "frame width 0 is not 1 to 480");
}
