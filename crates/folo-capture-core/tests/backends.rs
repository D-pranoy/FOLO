//! The synthetic and replay backends (ADR-0037 D22): the same pictures every
//! time, the trait's errors, and the replay file format.

use std::time::Duration;

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::codec;
use folo_capture_core::error::CaptureError;
use folo_capture_core::replay::{self, ReplayBackend};
use folo_capture_core::synthetic::{self, Pattern, SyntheticBackend};
use folo_capture_core::types::{
    Bars, BlackBarMode, BlackBarPreset, BlackBarSettings, CaptureRequest, FrameFlags, Picture,
    Profile, ScreenFrame,
};

const NO_WAIT: Duration = Duration::ZERO;

fn request(screen: &str, width: u16, height: u16) -> CaptureRequest {
    CaptureRequest {
        screen: screen.to_owned(),
        width,
        height,
        rate_hz: 60.0,
        black_bars: BlackBarSettings {
            preset: BlackBarPreset::Auto,
            mode: BlackBarMode::Default,
            threshold: 0.05,
        },
        profile: Profile::Video,
        restore_token: None,
    }
}

fn pictures(backend: &mut dyn CaptureBackend, count: usize) -> Vec<Picture> {
    (0..count)
        .map(|_| backend.next_picture(NO_WAIT).unwrap().unwrap())
        .collect()
}

fn recorded(index: u8, timestamp_us: u64) -> ScreenFrame {
    ScreenFrame {
        sequence: u32::from(index),
        timestamp_us,
        picture: Picture {
            screen_index: 0,
            width: 4,
            height: 2,
            flags: FrameFlags {
                protected: index == 2,
                hdr: false,
                held: false,
            },
            bars: Bars::default(),
            pixels: vec![index * 10; 24],
        },
    }
}

#[test]
fn synthetic_lists_one_screen_without_starting_anything() {
    let screens = SyntheticBackend::unpaced(Pattern::Gradient)
        .list_screens()
        .unwrap();
    assert_eq!(screens.len(), 1);
    assert_eq!(
        (
            screens[0].id.as_str(),
            screens[0].width,
            screens[0].height,
            screens[0].primary
        ),
        (synthetic::SCREEN_ID, 1920, 1080, true)
    );
}

#[test]
fn synthetic_frames_fit_the_box_in_the_screens_aspect_ratio() {
    let mut backend = SyntheticBackend::unpaced(Pattern::Gradient);
    backend
        .start(&request(synthetic::SCREEN_ID, 480, 270))
        .unwrap();
    let picture = backend.next_picture(NO_WAIT).unwrap().unwrap();
    assert_eq!((picture.width, picture.height), (480, 270));
    assert_eq!(picture.pixels.len(), 480 * 270 * 3);
    backend
        .start(&request(synthetic::SCREEN_ID, 100, 100))
        .unwrap();
    let picture = backend.next_picture(NO_WAIT).unwrap().unwrap();
    assert_eq!((picture.width, picture.height), (100, 56));
}

#[test]
fn two_synthetic_backends_give_the_same_pictures() {
    for pattern in [Pattern::Gradient, Pattern::ColorBars, Pattern::Letterbox] {
        let mut first = SyntheticBackend::unpaced(pattern);
        let mut second = SyntheticBackend::unpaced(pattern);
        first.start(&request(synthetic::SCREEN_ID, 64, 36)).unwrap();
        second
            .start(&request(synthetic::SCREEN_ID, 64, 36))
            .unwrap();
        assert_eq!(
            pictures(&mut first, 5),
            pictures(&mut second, 5),
            "{pattern:?}"
        );
    }
}

#[test]
fn the_gradient_moves_and_the_bars_do_not() {
    let mut gradient = SyntheticBackend::unpaced(Pattern::Gradient);
    gradient
        .start(&request(synthetic::SCREEN_ID, 64, 36))
        .unwrap();
    let moving = pictures(&mut gradient, 2);
    assert_ne!(moving[0].pixels, moving[1].pixels);
    let mut bars = SyntheticBackend::unpaced(Pattern::ColorBars);
    bars.start(&request(synthetic::SCREEN_ID, 64, 36)).unwrap();
    let still = pictures(&mut bars, 2);
    assert_eq!(still[0].pixels, still[1].pixels);
}

#[test]
fn the_gradient_has_exact_values_at_known_cells() {
    let picture = SyntheticBackend::render(Pattern::Gradient, 3, 5, 3);
    // Cell (x 4, y 2): red = (4 * 255 / 4 + 3 * 4) % 256, green = 2 * 255 / 2, blue = 3 * 8.
    let at = (2 * 5 + 4) * 3;
    assert_eq!(&picture.pixels[at..at + 3], &[11, 255, 24]);
    assert_eq!(&picture.pixels[0..3], &[12, 0, 24]);
}

#[test]
fn colour_bars_are_eight_bars_left_to_right() {
    let picture = SyntheticBackend::render(Pattern::ColorBars, 0, 8, 1);
    let bars: Vec<&[u8]> = picture.pixels.chunks(3).collect();
    assert_eq!(bars[0], [255, 255, 255]);
    assert_eq!(bars[3], [0, 255, 0]);
    assert_eq!(bars[5], [255, 0, 0]);
    assert_eq!(bars[7], [0, 0, 0]);
}

#[test]
fn the_letterbox_reports_the_black_rows_it_draws() {
    let picture = SyntheticBackend::render(Pattern::Letterbox, 0, 16, 16);
    assert_eq!(
        picture.bars,
        Bars {
            top: 2,
            bottom: 2,
            left: 0,
            right: 0
        }
    );
    assert!(picture.bars.fit(16, 16));
    let row = |y: usize| &picture.pixels[y * 16 * 3..(y + 1) * 16 * 3];
    assert!(row(0).iter().all(|&b| b == 0) && row(1).iter().all(|&b| b == 0));
    assert!(row(14).iter().all(|&b| b == 0) && row(15).iter().all(|&b| b == 0));
    assert!(row(2).iter().any(|&b| b != 0));
    // Every pattern makes frames the codec takes.
    for pattern in [Pattern::Gradient, Pattern::ColorBars, Pattern::Letterbox] {
        let picture = SyntheticBackend::render(pattern, 9, 480, 270);
        codec::encode(&ScreenFrame {
            sequence: 0,
            timestamp_us: 0,
            picture,
        })
        .unwrap();
    }
}

#[test]
fn the_trait_refuses_what_it_should() {
    let mut backend = SyntheticBackend::unpaced(Pattern::Gradient);
    assert_eq!(backend.next_picture(NO_WAIT), Err(CaptureError::NotStarted));
    assert_eq!(
        backend.start(&request("nowhere", 64, 36)),
        Err(CaptureError::UnknownScreen("nowhere".to_owned()))
    );
    assert!(matches!(
        backend.start(&request(synthetic::SCREEN_ID, 0, 36)),
        Err(CaptureError::InvalidRequest(_))
    ));
    backend
        .start(&request(synthetic::SCREEN_ID, 64, 36))
        .unwrap();
    backend.stop();
    assert_eq!(backend.next_picture(NO_WAIT), Err(CaptureError::NotStarted));
    backend.stop();
}

#[test]
fn starting_again_goes_on_in_place_with_the_new_size() {
    let mut backend = SyntheticBackend::unpaced(Pattern::Gradient);
    backend
        .start(&request(synthetic::SCREEN_ID, 64, 36))
        .unwrap();
    pictures(&mut backend, 3);
    backend
        .start(&request(synthetic::SCREEN_ID, 128, 72))
        .unwrap();
    let picture = backend.next_picture(NO_WAIT).unwrap().unwrap();
    assert_eq!((picture.width, picture.height), (128, 72));
    assert_eq!(
        picture,
        SyntheticBackend::render(Pattern::Gradient, 3, 128, 72)
    );
}

#[test]
fn a_paced_backend_returns_nothing_when_the_next_frame_is_further_off_than_the_timeout() {
    let mut backend = SyntheticBackend::new(Pattern::Gradient);
    let mut slow = request(synthetic::SCREEN_ID, 64, 36);
    slow.rate_hz = 1.0;
    backend.start(&slow).unwrap();
    assert!(
        backend
            .next_picture(Duration::from_millis(5))
            .unwrap()
            .is_some()
    );
    assert!(
        backend
            .next_picture(Duration::from_millis(5))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_replay_file_round_trips_and_plays_in_order_and_loops() {
    let frames = vec![recorded(1, 0), recorded(2, 10_000), recorded(3, 20_000)];
    let bytes = replay::write(&frames).unwrap();
    assert_eq!(&bytes[..8], b"FOLR\x01\0\0\0");
    assert_eq!(replay::parse(&bytes).unwrap(), frames);

    let mut backend = ReplayBackend::new(frames.clone(), false).unwrap();
    let screens = backend.list_screens().unwrap();
    assert_eq!(
        (screens[0].id.as_str(), screens[0].width, screens[0].height),
        ("replay", 4, 2)
    );
    backend.start(&request("replay", 64, 36)).unwrap();
    let played = pictures(&mut backend, 5);
    let expected: Vec<Picture> = [0, 1, 2, 0, 1]
        .iter()
        .map(|&i| frames[i].picture.clone())
        .collect();
    assert_eq!(played, expected);
    assert!(
        played[1].flags.protected,
        "recorded flags are played as they are"
    );
}

#[test]
fn a_replay_backend_follows_the_recorded_gaps() {
    let frames = vec![recorded(1, 0), recorded(2, 30_000)];
    let mut backend = ReplayBackend::new(frames, true).unwrap();
    backend.start(&request("replay", 64, 36)).unwrap();
    assert!(backend.next_picture(NO_WAIT).unwrap().is_some());
    // The second frame is 30 ms after the first.
    assert!(
        backend
            .next_picture(Duration::from_millis(1))
            .unwrap()
            .is_none()
    );
    assert!(
        backend
            .next_picture(Duration::from_millis(500))
            .unwrap()
            .is_some()
    );
}

#[test]
fn replay_refuses_a_wrong_screen_a_missing_start_and_no_frames() {
    let mut backend = ReplayBackend::new(vec![recorded(1, 0)], false).unwrap();
    assert_eq!(backend.next_picture(NO_WAIT), Err(CaptureError::NotStarted));
    assert_eq!(
        backend.start(&request("other", 64, 36)),
        Err(CaptureError::UnknownScreen("other".to_owned()))
    );
    assert!(ReplayBackend::new(Vec::new(), false).is_err());
    assert!(replay::write(&[]).is_err());
}

#[test]
fn a_malformed_replay_file_is_an_error_at_every_cut_and_in_every_header_byte() {
    let bytes = replay::write(&[recorded(1, 0), recorded(2, 5)]).unwrap();
    for length in 0..bytes.len() {
        let cut = replay::parse(&bytes[..length]);
        // A cut exactly after the first record is a shorter, valid file.
        if length != 8 + 4 + codec::message_len(4, 2) {
            assert!(cut.is_err(), "{length} bytes");
        }
    }
    let mut wrong = bytes.clone();
    wrong[0] = b'X';
    assert!(replay::parse(&wrong).is_err());
    wrong = bytes.clone();
    wrong[4] = 2;
    assert!(replay::parse(&wrong).is_err());
    wrong = bytes.clone();
    wrong[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(replay::parse(&wrong).is_err());
    wrong[8..12].copy_from_slice(&35_u32.to_le_bytes());
    assert!(replay::parse(&wrong).is_err());
    assert!(
        replay::parse(b"FOLR\x01\0\0\0").is_err(),
        "a file without frames"
    );
}

#[test]
fn a_replay_file_is_read_from_disk() {
    let dir = std::env::temp_dir().join(format!("folo-capture-core-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("two.folr");
    std::fs::write(
        &path,
        replay::write(&[recorded(1, 0), recorded(2, 5)]).unwrap(),
    )
    .unwrap();
    let mut backend = ReplayBackend::open(&path).unwrap();
    assert_eq!(backend.name(), "replay");
    assert_eq!(backend.list_screens().unwrap()[0].width, 4);
    let missing = ReplayBackend::open(&dir.join("missing.folr"));
    assert!(matches!(missing, Err(CaptureError::Failed(_))));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_synthetic_picture_of_a_degenerate_size_does_not_panic() {
    for (width, height) in [(0, 0), (1, 1), (1, 7), (7, 1)] {
        for pattern in [Pattern::Gradient, Pattern::ColorBars, Pattern::Letterbox] {
            let picture = SyntheticBackend::render(pattern, 0, width, height);
            assert_eq!(
                picture.pixels.len(),
                usize::from(width.max(1)) * usize::from(height.max(1)) * 3
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_replay_path_that_is_not_a_regular_file_is_refused_without_reading_it() {
    let result = ReplayBackend::open(std::path::Path::new("/dev/zero"));
    assert!(matches!(result, Err(CaptureError::Failed(why)) if why.contains("not a regular file")));
}

#[test]
fn a_replay_file_over_the_cap_is_refused() {
    let dir = std::env::temp_dir().join(format!("folo-capture-core-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.folr");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(replay::MAX_FILE_BYTES + 1).unwrap();
    let result = ReplayBackend::open(&path);
    assert!(matches!(result, Err(CaptureError::Failed(why)) if why.contains("longer than 64 MiB")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_report_hides_its_restore_token_when_debug_printed() {
    use folo_capture_core::backend::BackendReport;
    use folo_capture_core::types::CaptureState;
    let report = BackendReport {
        state: CaptureState::Capturing,
        message: None,
        restore_token: Some("token-that-must-not-be-logged".to_owned()),
        fps: 30.0,
    };
    let shown = format!("{report:?}");
    assert!(
        shown.contains("Capturing") && shown.contains("<hidden>"),
        "{shown}"
    );
    assert!(!shown.contains("must-not-be-logged"), "{shown}");
}
