//! Raw frames reach no log or status (ADR-0037 D9): nothing that carries
//! pixels prints them with `{:?}`, and errors name fields only.

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::codec;
use folo_capture_core::error::{CaptureError, FrameError};
use folo_capture_core::pacer::FramePacer;
use folo_capture_core::replay::{self, ReplayBackend};
use folo_capture_core::types::{Bars, FrameFlags, Picture, ScreenFrame};

/// Pixel bytes whose decimal and hex spellings are easy to find in a dump.
const MARKER: [u8; 3] = [201, 77, 143];

fn marked_picture() -> Picture {
    Picture {
        screen_index: 3,
        width: 4,
        height: 2,
        flags: FrameFlags {
            protected: true,
            hdr: false,
            held: true,
        },
        bars: Bars {
            top: 1,
            bottom: 0,
            left: 0,
            right: 1,
        },
        pixels: MARKER.repeat(8),
    }
}

fn marked_frame() -> ScreenFrame {
    ScreenFrame {
        sequence: 42,
        timestamp_us: 1_234_567,
        picture: marked_picture(),
    }
}

/// Fails when a dump holds the marker, in any spelling a `Debug` could use.
fn assert_no_pixels(dump: &str) {
    for spelling in [
        "201, 77, 143",
        "201,77,143",
        "0xc9",
        "c9, 4d, 8f",
        "[201",
        "201, 77",
    ] {
        assert!(!dump.contains(spelling), "{spelling:?} in {dump}");
    }
    assert!(!dump.contains("pixels:"), "{dump}");
}

#[test]
fn a_picture_prints_its_metadata_and_the_length_of_its_pixels_only() {
    let dump = format!("{:?}", marked_picture());
    assert_no_pixels(&dump);
    assert_eq!(
        dump,
        "Picture { screen_index: 3, width: 4, height: 2, flags: FrameFlags { protected: true, hdr: false, held: true }, \
         bars: Bars { top: 1, bottom: 0, left: 0, right: 1 }, pixel_bytes: 24 }"
    );
}

#[test]
fn a_frame_prints_its_sequence_time_and_picture_without_pixels() {
    let frame = marked_frame();
    for dump in [format!("{frame:?}"), format!("{frame:#?}")] {
        assert_no_pixels(&dump);
        assert!(
            dump.contains("sequence: 42") && dump.contains("timestamp_us: 1234567"),
            "{dump}"
        );
        assert!(dump.contains("pixel_bytes: 24"), "{dump}");
    }
}

#[test]
fn what_holds_frames_prints_no_pixels_either() {
    let mut pacer = FramePacer::new(60.0);
    pacer.push(marked_picture());
    assert_no_pixels(&format!("{pacer:?}"));
    pacer.poll(0).unwrap();
    pacer.push(marked_picture());
    assert_no_pixels(&format!("{pacer:#?}"));

    let backend = ReplayBackend::new(vec![marked_frame(), marked_frame()], false).unwrap();
    assert_no_pixels(&format!("{backend:?}"));
    let mut started = backend;
    started.stop();
    assert_no_pixels(&format!("{started:#?}"));
}

#[test]
fn errors_and_encoded_frames_name_fields_and_never_print_pixels() {
    let mut short = marked_frame();
    short.picture.pixels.pop();
    let error = codec::encode(&short).unwrap_err();
    assert_eq!(
        error,
        FrameError::BadPixels {
            expected: 24,
            actual: 23
        }
    );
    assert_no_pixels(&format!("{error:?} {error}"));

    let mut bytes = codec::encode(&marked_frame()).unwrap();
    bytes[4] = 9;
    let error = codec::decode(&bytes).unwrap_err();
    assert_no_pixels(&format!("{error:?} {error}"));
    let wrapped = CaptureError::from(error);
    assert_no_pixels(&format!("{wrapped:?} {wrapped}"));

    let file = replay::write(&[marked_frame()]).unwrap();
    let broken = replay::parse(&file[..file.len() - 1]).unwrap_err();
    assert_no_pixels(&format!("{broken:?} {broken}"));
}
