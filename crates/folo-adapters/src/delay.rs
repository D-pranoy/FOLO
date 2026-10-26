//! Stage 4: the latency delay line (ADR-0036 D17).

use std::collections::VecDeque;

use folo_provider::ElementColors;

/// The longest delay, 5 s.
pub const MAX_DELAY_MS: f64 = 5000.0;

/// Holds each frame back by a fixed time.
///
/// Until the line has aged by the delay, it shows its first frame, as if it had been filled with
/// it: the lights start with a picture rather than with black.
pub(crate) struct DelayLine {
    delay_us: u64,
    /// Frames oldest first. The first is the newest that is at least `delay_us` old, or, before
    /// any is, the first frame ever.
    frames: VecDeque<(u64, ElementColors)>,
    /// The newest time seen, so a clock that steps back doesn't reorder the line.
    latest_us: u64,
}

impl DelayLine {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn new(delay_ms: f64) -> Self {
        let ms = if delay_ms.is_finite() {
            delay_ms.clamp(0.0, MAX_DELAY_MS)
        } else {
            0.0
        };
        Self {
            delay_us: (ms * 1000.0).round() as u64,
            frames: VecDeque::new(),
            latest_us: 0,
        }
    }

    /// Whether the line passes frames on as they come.
    pub(crate) fn is_none(&self) -> bool {
        self.delay_us == 0
    }

    /// Takes the frame of `now_us` in and gives the one that is due out.
    pub(crate) fn run(&mut self, now_us: u64, colors: ElementColors) -> ElementColors {
        if self.delay_us == 0 {
            return colors;
        }
        let now_us = now_us.max(self.latest_us);
        self.latest_us = now_us;
        self.frames.push_back((now_us, colors));
        let due = now_us.saturating_sub(self.delay_us);
        // Drop what a newer frame that is already due has replaced.
        while self.frames.len() > 1 && self.frames[1].0 <= due {
            self.frames.pop_front();
        }
        self.frames[0].1.clone()
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;

    fn dimmer(v: f32) -> ElementColors {
        ElementColors::Dimmer(vec![v])
    }

    #[test]
    fn no_delay_passes_frames_through() {
        let mut line = DelayLine::new(0.0);
        assert!(line.is_none());
        assert_eq!(line.run(5, dimmer(1.0)), dimmer(1.0));
        assert_eq!(line.run(6, dimmer(2.0)), dimmer(2.0));
        assert!(line.frames.is_empty());
    }

    #[test]
    fn frames_come_out_after_the_delay() {
        // 100 ms, frames every 40 ms.
        let mut line = DelayLine::new(100.0);
        let mut out = Vec::new();
        for i in 0..8u64 {
            out.push(line.run(i * 40_000, dimmer(i as f32)));
        }
        // t=0..80 ms: nothing is 100 ms old yet, the first frame shows.
        assert!(out[..3].iter().all(|c| *c == dimmer(0.0)));
        // t=120 ms: the frame of t=0 is due; t=160: t=40's, then t=80's (a 100 ms-old frame is due).
        assert_eq!(out[3], dimmer(0.0));
        assert_eq!(out[4], dimmer(1.0));
        assert_eq!(out[5], dimmer(2.0));
        assert_eq!(out[6], dimmer(3.0));
        assert_eq!(out[7], dimmer(4.0));
    }

    #[test]
    fn a_step_in_the_input_arrives_exactly_one_delay_later() {
        let mut line = DelayLine::new(250.0);
        let mut step_seen = None;
        for i in 0..60u64 {
            let t = i * 10_000;
            let input = if t >= 100_000 { 1.0 } else { 0.0 };
            if line.run(t, dimmer(input)) == dimmer(1.0) && step_seen.is_none() {
                step_seen = Some(t);
            }
        }
        assert_eq!(step_seen, Some(350_000), "100 ms + 250 ms");
    }

    #[test]
    fn a_stalled_clock_keeps_the_line_in_order() {
        let mut line = DelayLine::new(100.0);
        line.run(1_000_000, dimmer(1.0));
        // The clock goes back: the frame counts as being from the newest time.
        assert_eq!(line.run(900_000, dimmer(2.0)), dimmer(1.0));
        assert_eq!(line.run(1_100_000, dimmer(3.0)), dimmer(2.0));
    }

    #[test]
    fn the_line_keeps_only_what_the_delay_needs() {
        let mut line = DelayLine::new(200.0);
        for i in 0..1000u64 {
            line.run(i * 16_667, dimmer(0.0));
        }
        // 200 ms at 60 Hz is about 12 frames, plus the one that is due.
        assert!(line.frames.len() <= 14, "{}", line.frames.len());
    }

    #[test]
    fn the_delay_is_limited_to_five_seconds() {
        assert_eq!(DelayLine::new(60_000.0).delay_us, 5_000_000);
        assert_eq!(DelayLine::new(-5.0).delay_us, 0);
        assert_eq!(DelayLine::new(f64::NAN).delay_us, 0);
    }
}
