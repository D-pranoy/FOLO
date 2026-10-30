//! Keeps the GPU work to the rate asked for (ADR-0037 D5): a desktop that
//! changes at 144 Hz is downscaled at most `rate_hz` times a second. Time
//! comes in as arguments, so it is tested without waiting.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct RateGate {
    interval_us: u64,
    last_us: Option<u64>,
}

impl RateGate {
    /// A gate that lets through `rate_hz` pictures a second (1 to 60).
    #[must_use]
    pub fn new(rate_hz: f64) -> Self {
        let mut gate = Self {
            interval_us: 0,
            last_us: None,
        };
        gate.set_rate(rate_hz);
        gate
    }

    /// Changes the rate in place.
    pub fn set_rate(&mut self, rate_hz: f64) {
        let rate = if rate_hz.is_nan() {
            1.0
        } else {
            rate_hz.clamp(1.0, 60.0)
        };
        // 1e6 / 60 up to 1e6 / 1: the cast can't overflow or lose the sign.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let interval = (1_000_000.0 / rate).floor() as u64;
        self.interval_us = interval;
    }

    /// How long until the next picture may be made; zero when it may be made now.
    #[must_use]
    pub fn wait(&self, now_us: u64) -> Duration {
        let ready_at = self
            .last_us
            .map_or(0, |last| last.saturating_add(self.interval_us));
        Duration::from_micros(ready_at.saturating_sub(now_us))
    }

    /// A picture was made at `now_us`.
    pub fn mark(&mut self, now_us: u64) {
        self.last_us = Some(now_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_picture_may_be_made_at_once_and_the_next_after_the_interval() {
        let mut gate = RateGate::new(30.0);
        assert_eq!(gate.wait(5), Duration::ZERO);
        gate.mark(1_000_000);
        assert_eq!(gate.wait(1_000_000), Duration::from_micros(33_333));
        assert_eq!(gate.wait(1_020_000), Duration::from_micros(13_333));
        assert_eq!(gate.wait(1_033_333), Duration::ZERO);
        assert_eq!(gate.wait(2_000_000), Duration::ZERO);
    }

    #[test]
    fn the_rate_changes_in_place_and_stays_within_one_to_sixty() {
        let mut gate = RateGate::new(60.0);
        gate.mark(0);
        assert_eq!(gate.wait(0), Duration::from_micros(16_666));
        gate.set_rate(1.0);
        assert_eq!(gate.wait(0), Duration::from_secs(1));
        gate.set_rate(1000.0);
        assert_eq!(gate.wait(0), Duration::from_micros(16_666));
        gate.set_rate(f64::NAN);
        assert_eq!(gate.wait(0), Duration::from_secs(1));
    }
}
