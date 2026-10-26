//! The clock that stamps frames (ADR-0037 D6): microseconds on
//! `QueryPerformanceCounter`, the counter the engine's `time.monotonic_ns`
//! reads (Python 3.13), so the agent's and the engine's stamps compare.

use folo_capture_core::clock::Clock;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// Microseconds of the performance counter since the system started.
#[derive(Debug, Clone, Copy)]
pub struct QpcClock {
    ticks_per_second: u64,
}

impl QpcClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            // The frequency is fixed at boot and never 0 on Windows XP and later.
            ticks_per_second: Self::read_frequency().max(1),
        }
    }

    #[allow(unsafe_code)] // one Win32 call with an out pointer
    fn read_frequency() -> u64 {
        let mut frequency = 0_i64;
        // SAFETY: the pointer is to a local that lives for the call.
        let _ = unsafe { QueryPerformanceFrequency(&raw mut frequency) };
        u64::try_from(frequency).unwrap_or(0)
    }

    #[allow(unsafe_code)] // one Win32 call with an out pointer
    fn read_counter() -> u64 {
        let mut ticks = 0_i64;
        // SAFETY: the pointer is to a local that lives for the call.
        let _ = unsafe { QueryPerformanceCounter(&raw mut ticks) };
        u64::try_from(ticks).unwrap_or(0)
    }

    /// Converts ticks to microseconds without overflowing for a counter that has run for years.
    #[must_use]
    pub fn micros_of(&self, ticks: u64) -> u64 {
        let micros = u128::from(ticks) * 1_000_000 / u128::from(self.ticks_per_second);
        u64::try_from(micros).unwrap_or(u64::MAX)
    }
}

impl Default for QpcClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for QpcClock {
    fn now_us(&self) -> u64 {
        self.micros_of(Self::read_counter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_become_microseconds_at_the_counters_frequency() {
        let clock = QpcClock {
            ticks_per_second: 10_000_000,
        };
        assert_eq!(clock.micros_of(10_000_000), 1_000_000);
        assert_eq!(clock.micros_of(15), 1);
        // Ten years of ticks at 10 MHz is far beyond what a u64 of ticks times 1e6 holds.
        assert_eq!(clock.micros_of(3_153_600_000_000_000), 315_360_000_000_000);
    }

    #[test]
    fn the_clock_is_monotonic_and_moves() {
        let clock = QpcClock::new();
        let first = clock.now_us();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = clock.now_us();
        assert!(second >= first + 15_000, "{first} then {second}");
    }
}
