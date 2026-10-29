//! The microsecond clock that stamps frames (ADR-0037 D6).

use std::time::Instant;

/// Microseconds on a monotonic clock.
pub trait Clock: Send {
    fn now_us(&self) -> u64;
}

/// Microseconds since the clock was made, on [`Instant`]. Safe code can't
/// read the operating system's raw counter (`QueryPerformanceCounter`,
/// `CLOCK_MONOTONIC`) that D6 names and the engine's clock reads, so this is
/// the clock of the synthetic and replay backends and of tests. The backend
/// crates bring the real one with their FFI.
#[derive(Debug, Clone, Copy)]
pub struct ProcessClock {
    start: Instant,
}

impl ProcessClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for ProcessClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ProcessClock {
    fn now_us(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
}
