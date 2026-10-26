//! When a frame's border becomes the one in force: Hyperion.NG's
//! `BlackBorderProcessor::updateBorder`, counting time instead of frames
//! (ADR-0037 D7), because the capture rate varies and a repeated frame costs
//! nothing.
//!
//! A port of Hyperion.NG (Copyright (c) 2014-2026 Hyperion Project, MIT; the
//! notice is in `THIRD_PARTY_NOTICES`, ADR-0023 D4).
//!
//! Hyperion.NG's frame counts are its defaults for a 25 frames a second
//! capture: 50 frames to switch to a border, 600 to give up on one, and 10
//! differing frames tolerated. The durations below are those counts at 25
//! frames a second for the video profile.

use std::time::Duration;

use super::scan::Border;
use crate::types::Profile;

/// How long things take, for one profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// A new border is adopted after being seen this long without a
    /// different one lasting (`borderFrameCnt`).
    pub border_hold: Duration,
    /// "Unknown" is adopted after being seen this long (`unknownFrameCnt`).
    pub unknown_hold: Duration,
    /// Frames that differ from the border being watched are ignored for this
    /// long (`maxInconsistentCnt`).
    pub tolerance: Duration,
}

impl Timing {
    /// The timing of a profile. `game` adopts changes faster, as a game's
    /// picture changes more often.
    #[must_use]
    pub fn of(profile: Profile) -> Self {
        match profile {
            Profile::Video => Self {
                border_hold: Duration::from_secs(2),
                unknown_hold: Duration::from_secs(24),
                tolerance: Duration::from_millis(400),
            },
            Profile::Game => Self {
                border_hold: Duration::from_millis(500),
                unknown_hold: Duration::from_secs(6),
                tolerance: Duration::from_millis(100),
            },
        }
    }
}

/// How long the frames have differed from the border being watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Differing {
    /// The latest frame matched it.
    Never,
    /// Since this time (microseconds), and still tolerated.
    Since(u64),
    /// Past the tolerance: the next differing frame replaces it at once.
    Past,
}

/// The border in force, and the one being watched to replace it.
#[derive(Debug, Clone)]
pub struct Hysteresis {
    timing: Timing,
    current: Border,
    watched: Border,
    /// When `watched` was first seen (microseconds).
    watched_since_us: u64,
    differing: Differing,
}

impl Hysteresis {
    /// Starts with no border known. As in Hyperion.NG, the first border
    /// found is adopted at once.
    #[must_use]
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            current: Border::UNKNOWN,
            watched: Border::UNKNOWN,
            watched_since_us: 0,
            differing: Differing::Past,
        }
    }

    /// Changes the timing; what was seen so far stays.
    pub fn set_timing(&mut self, timing: Timing) {
        self.timing = timing;
    }

    /// The border in force.
    #[must_use]
    pub fn current(&self) -> Border {
        self.current
    }

    /// Takes the border of a frame seen at `now_us`; whether the border in
    /// force changed.
    pub fn update(&mut self, seen: Border, now_us: u64) -> bool {
        if seen.same_as(&self.watched) {
            self.differing = Differing::Never;
        } else if !self.keep_watching(now_us) {
            self.watched = seen;
            self.watched_since_us = now_us;
        } else {
            return false;
        }
        if seen.same_as(&self.current) {
            self.differing = Differing::Never;
            return false;
        }
        let steady = Duration::from_micros(now_us.saturating_sub(self.watched_since_us));
        let adopt = if seen.unknown {
            steady >= self.timing.unknown_hold
        } else {
            self.current.unknown || steady >= self.timing.border_hold
        };
        if adopt {
            self.current = seen;
        }
        adopt
    }

    /// A frame differing from the watched border: whether it is still within
    /// the tolerance (and so ignored).
    fn keep_watching(&mut self, now_us: u64) -> bool {
        match self.differing {
            Differing::Never => {
                self.differing = Differing::Since(now_us);
                true
            }
            Differing::Since(since) => {
                let lasted = Duration::from_micros(now_us.saturating_sub(since));
                if lasted <= self.timing.tolerance {
                    true
                } else {
                    self.differing = Differing::Past;
                    false
                }
            }
            Differing::Past => false,
        }
    }
}
