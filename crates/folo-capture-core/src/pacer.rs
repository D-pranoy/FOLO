//! What sits between a backend and the link (ADR-0037 D5, D13): it numbers
//! and stamps the pictures, keeps them to the rate asked for, and repeats the
//! last picture when nothing new came, with `held` set while the backend
//! can't capture. Time comes in as arguments, so it is tested without waiting.

use std::time::Duration;

use crate::types::{Picture, ScreenFrame};

/// The last picture is sent again after this long without a new one (D5).
pub const REPEAT_AFTER: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct FramePacer {
    interval_us: u64,
    sequence: u32,
    /// The newest picture the backend gave that isn't sent yet.
    pending: Option<Picture>,
    /// The last picture sent, as it was.
    last: Option<Picture>,
    last_sent_us: Option<u64>,
    held: bool,
}

impl FramePacer {
    /// A pacer for at most `rate_hz` frames a second (1 to 60).
    #[must_use]
    pub fn new(rate_hz: f64) -> Self {
        let mut pacer = Self {
            interval_us: 0,
            sequence: 0,
            pending: None,
            last: None,
            last_sent_us: None,
            held: false,
        };
        pacer.set_rate(rate_hz);
        pacer
    }

    /// Makes the first frame take this sequence number, for tests of the wrap.
    #[must_use]
    pub fn starting_at(mut self, sequence: u32) -> Self {
        self.sequence = sequence;
        self
    }

    /// Changes the rate in place, as a repeated `capture.start` does. The
    /// sequence goes on.
    pub fn set_rate(&mut self, rate_hz: f64) {
        let rate = rate_hz.clamp(1.0, 60.0);
        // 1e6 / 60 up to 1e6 / 1: the cast can't overflow or lose the sign.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let interval = (1_000_000.0 / rate).floor() as u64;
        self.interval_us = interval;
    }

    /// Whether the backend can't capture now (the secure desktop, a mode
    /// change, a disconnected session): the repeats then carry `held`.
    pub fn set_held(&mut self, held: bool) {
        self.held = held;
    }

    /// Takes a new picture. Of two pictures that wait for the rate limit, the
    /// newer one is kept (newest wins).
    pub fn push(&mut self, picture: Picture) {
        self.pending = Some(picture);
    }

    /// The frame to send at `now_us`, if one is due.
    ///
    /// A pending picture goes out once `interval` has passed since the last
    /// frame. With none, the last picture goes out again after
    /// [`REPEAT_AFTER`] since the last frame; it carries `held` while
    /// [`set_held`](Self::set_held) says so. Every frame takes the next
    /// sequence number and `now_us` as its time.
    pub fn poll(&mut self, now_us: u64) -> Option<ScreenFrame> {
        let since_last = self.last_sent_us.map(|sent| now_us.saturating_sub(sent));
        let picture = if let Some(pending) = self
            .pending
            .take_if(|_| since_last.is_none_or(|s| s >= self.interval_us))
        {
            pending
        } else if self.pending.is_none()
            && since_last.is_some_and(|s| u128::from(s) >= REPEAT_AFTER.as_micros())
        {
            self.last.clone()?
        } else {
            return None;
        };
        self.last = Some(picture.clone());
        Some(self.stamp(picture, now_us))
    }

    fn stamp(&mut self, mut picture: Picture, now_us: u64) -> ScreenFrame {
        picture.flags.held |= self.held;
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.last_sent_us = Some(now_us);
        ScreenFrame {
            sequence,
            timestamp_us: now_us,
            picture,
        }
    }
}
