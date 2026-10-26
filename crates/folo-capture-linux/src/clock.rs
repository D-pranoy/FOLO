//! The clock that stamps frames on Linux (ADR-0037 D6).

use folo_capture_core::clock::Clock;
use rustix::time::{ClockId, clock_gettime};

/// Microseconds on `CLOCK_MONOTONIC`, the counter the engine's
/// `time.monotonic_ns` reads, so the agent's stamps and the engine's compare
/// directly. `rustix` makes the call, so this is safe code (D19).
#[derive(Debug, Clone, Copy, Default)]
pub struct MonotonicClock;

impl Clock for MonotonicClock {
    fn now_us(&self) -> u64 {
        let now = clock_gettime(ClockId::Monotonic);
        // The clock doesn't go below 0, and the nanoseconds are under 10^9.
        let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
        let nanoseconds = u64::try_from(now.tv_nsec).unwrap_or(0);
        seconds
            .saturating_mul(1_000_000)
            .saturating_add(nanoseconds / 1000)
    }
}
