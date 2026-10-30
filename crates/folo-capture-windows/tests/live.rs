//! Live checks of the three backends on the real desktop of the signed-in
//! user (ADR-0037 D22, [testing policy](../../../docs/process/testing-policy.md)).
//! They capture the screen, so CI never runs them (`#[ignore]`): a maintainer
//! runs them on a real Windows session with
//! `cargo test -p folo-capture-windows --test live -- --ignored --test-threads=1`,
//! in SDR and again with the desktop in HDR, and on a hybrid-GPU laptop.
//! Nothing is saved or printed of the pictures; only sizes and flags are.
#![cfg(windows)]

use std::time::Duration;

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::types::{
    BlackBarMode, BlackBarPreset, BlackBarSettings, CaptureRequest, Profile,
};
use folo_capture_windows::factory::backend;
use folo_capture_windows::recovery::BackendKind;

fn request(screen: &str) -> CaptureRequest {
    CaptureRequest {
        screen: screen.to_owned(),
        width: 480,
        height: 270,
        rate_hz: 30.0,
        black_bars: BlackBarSettings {
            preset: BlackBarPreset::Auto,
            mode: BlackBarMode::Default,
            threshold: 0.05,
        },
        profile: Profile::Video,
        restore_token: None,
    }
}

fn capture_a_few_pictures(order: Vec<BackendKind>) {
    let mut capture = backend(order);
    let screens = capture.list_screens().expect("the monitors list");
    let primary = screens
        .iter()
        .find(|screen| screen.primary)
        .or(screens.first())
        .expect("a monitor to capture");
    capture
        .start(&request(&primary.id))
        .expect("the capture starts");
    let mut got = 0;
    for _ in 0..200 {
        if let Some(picture) = capture
            .next_picture(Duration::from_millis(100))
            .expect("no unrecoverable failure")
        {
            assert!(picture.width <= 480 && picture.height <= 270);
            assert_eq!(
                picture.pixels.len(),
                usize::from(picture.width) * usize::from(picture.height) * 3
            );
            println!(
                "{}: {}x{}, hdr {}, protected {}",
                capture.name(),
                picture.width,
                picture.height,
                picture.flags.hdr,
                picture.flags.protected
            );
            got += 1;
            if got == 5 {
                break;
            }
        }
    }
    capture.stop();
    assert_eq!(
        got,
        5,
        "five pictures arrived; status {:?}",
        capture.status()
    );
}

#[test]
#[ignore = "captures the real screen: a live check"]
fn desktop_duplication_captures_the_primary_monitor() {
    capture_a_few_pictures(vec![BackendKind::Dda]);
}

#[test]
#[ignore = "captures the real screen: a live check"]
fn windows_graphics_capture_captures_the_primary_monitor() {
    capture_a_few_pictures(vec![BackendKind::Wgc]);
}

#[test]
#[ignore = "captures the real screen: a live check"]
fn gdi_captures_the_primary_monitor() {
    capture_a_few_pictures(vec![BackendKind::Gdi]);
}

#[test]
#[ignore = "captures the real screen: a live check"]
fn auto_captures_the_primary_monitor() {
    capture_a_few_pictures(BackendKind::auto_order());
}
