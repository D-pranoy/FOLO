//! The frame clock (ADR-0037 D6).
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use folo_capture_core::clock::Clock;
use folo_capture_linux::MonotonicClock;

#[test]
fn it_advances_with_the_monotonic_clock_in_microseconds() {
    let clock = MonotonicClock;
    let (before, started) = (clock.now_us(), Instant::now());
    std::thread::sleep(Duration::from_millis(60));
    let (after, elapsed) = (clock.now_us(), started.elapsed());
    let advanced = Duration::from_micros(after - before);
    assert!(after > before);
    // `Instant` reads the same counter on Linux, so the two agree closely.
    let difference = advanced.abs_diff(elapsed);
    assert!(
        difference < Duration::from_millis(5),
        "{advanced:?} vs {elapsed:?}"
    );
}

#[test]
fn it_reads_the_systems_counter_not_the_processs() {
    // CLOCK_MONOTONIC counts from boot: far more than the microseconds since
    // this process began.
    let boot_us = MonotonicClock.now_us();
    assert!(boot_us > 1_000_000, "{boot_us}");
}
