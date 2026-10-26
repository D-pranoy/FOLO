//! The pacer (ADR-0037 D5, D13): numbers and stamps frames, keeps to the
//! rate, repeats the last picture after 0.25 s, and marks repeats `held`.

use folo_capture_core::pacer::FramePacer;
use folo_capture_core::types::{Bars, FrameFlags, Picture};

fn picture(shade: u8) -> Picture {
    Picture {
        screen_index: 0,
        width: 2,
        height: 1,
        flags: FrameFlags::default(),
        bars: Bars::default(),
        pixels: vec![shade; 6],
    }
}

const MS: u64 = 1000;

#[test]
fn the_first_picture_goes_out_at_once_with_sequence_0_and_the_time_it_is_sent() {
    let mut pacer = FramePacer::new(60.0);
    assert!(pacer.poll(5 * MS).is_none());
    pacer.push(picture(1));
    let frame = pacer.poll(5 * MS).unwrap();
    assert_eq!((frame.sequence, frame.timestamp_us), (0, 5 * MS));
    assert_eq!(frame.picture, picture(1));
}

#[test]
fn pictures_faster_than_the_rate_wait_and_the_newest_wins() {
    let mut pacer = FramePacer::new(10.0);
    pacer.push(picture(1));
    assert!(pacer.poll(0).is_some());
    pacer.push(picture(2));
    pacer.push(picture(3));
    assert!(pacer.poll(99 * MS).is_none());
    let frame = pacer.poll(100 * MS).unwrap();
    assert_eq!(frame.picture, picture(3));
    assert_eq!(frame.sequence, 1);
    assert!(pacer.poll(100 * MS).is_none());
}

#[test]
fn the_rate_of_60_allows_a_frame_every_16_667_microseconds() {
    let mut pacer = FramePacer::new(60.0);
    pacer.push(picture(1));
    pacer.poll(0).unwrap();
    pacer.push(picture(2));
    assert!(pacer.poll(16_665).is_none());
    assert!(pacer.poll(16_666).is_some());
}

#[test]
fn a_rate_outside_1_to_60_is_taken_to_the_nearest_limit() {
    let mut fast = FramePacer::new(1000.0);
    fast.push(picture(1));
    fast.poll(0).unwrap();
    fast.push(picture(2));
    assert!(fast.poll(16_000).is_none());
    let mut slow = FramePacer::new(0.0);
    slow.push(picture(1));
    slow.poll(0).unwrap();
    slow.push(picture(2));
    assert!(slow.poll(999_999).is_none());
    assert!(slow.poll(1_000_000).is_some());
}

#[test]
fn a_still_screen_repeats_its_last_picture_after_a_quarter_second_as_a_new_frame() {
    let mut pacer = FramePacer::new(60.0);
    pacer.push(picture(9));
    pacer.poll(1_000 * MS).unwrap();
    assert!(pacer.poll(1_249 * MS).is_none());
    let repeat = pacer.poll(1_250 * MS).unwrap();
    assert_eq!((repeat.sequence, repeat.timestamp_us), (1, 1_250 * MS));
    assert_eq!(repeat.picture, picture(9));
    assert!(!repeat.picture.flags.held);
    // The next repeat counts from this one.
    assert!(pacer.poll(1_499 * MS).is_none());
    assert_eq!(pacer.poll(1_500 * MS).unwrap().sequence, 2);
}

#[test]
fn a_held_pacer_repeats_with_the_held_flag_and_a_new_picture_goes_out_unmarked_after_release() {
    let mut pacer = FramePacer::new(60.0);
    let mut protected = picture(4);
    protected.flags.protected = true;
    pacer.push(protected);
    pacer.poll(0).unwrap();
    pacer.set_held(true);
    let held = pacer.poll(250 * MS).unwrap();
    assert!(held.picture.flags.held && held.picture.flags.protected);
    assert_eq!(held.picture.pixels, vec![4; 6]);
    pacer.set_held(false);
    let again = pacer.poll(500 * MS).unwrap();
    assert!(
        !again.picture.flags.held,
        "held is not kept once the backend captures again"
    );
}

#[test]
fn a_new_picture_pushed_while_held_still_carries_held() {
    let mut pacer = FramePacer::new(60.0);
    pacer.set_held(true);
    pacer.push(picture(1));
    assert!(pacer.poll(0).unwrap().picture.flags.held);
}

#[test]
fn nothing_is_repeated_before_a_first_picture() {
    let mut pacer = FramePacer::new(60.0);
    pacer.set_held(true);
    assert!(pacer.poll(10_000 * MS).is_none());
}

#[test]
fn the_sequence_wraps_at_2_to_the_32() {
    let mut pacer = FramePacer::new(60.0).starting_at(u32::MAX);
    pacer.push(picture(1));
    assert_eq!(pacer.poll(0).unwrap().sequence, u32::MAX);
    assert_eq!(pacer.poll(250 * MS).unwrap().sequence, 0);
    assert_eq!(pacer.poll(500 * MS).unwrap().sequence, 1);
}

#[test]
fn changing_the_rate_in_place_keeps_the_sequence() {
    let mut pacer = FramePacer::new(60.0);
    pacer.push(picture(1));
    pacer.poll(0).unwrap();
    pacer.set_rate(1.0);
    pacer.push(picture(2));
    assert!(pacer.poll(500 * MS).is_none());
    assert_eq!(pacer.poll(1_000 * MS).unwrap().sequence, 1);
}
