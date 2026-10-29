//! Stage 5: the flash limiter (ADR-0036 D18, `docs/architecture/capabilities.md#algorithm`).
//!
//! WCAG 2.3.1's limit of three flashes in any second, per element, on what the lights will
//! show. A limiter that fails to act while it is on counts as a security problem (ADR-0033 D6),
//! so this file follows the text of the algorithm closely and says where it had to choose.
//!
//! # What the documents fix
//!
//! - **A flash is a pair.** A transition *opens* it, the first transition that opposes that one
//!   *completes* it, and the next transition opens the next. A transition in the direction the
//!   last one went (a slow rise counted in several steps of 0.10) changes nothing. A 3 Hz blink
//!   is three flashes a second and untouched; a 4 Hz blink is held. See [`Element::count`]. The
//!   transition that opens a fourth flash isn't held, only the one that completes it.
//! - **A run ends at a reversal of 0.02** from its furthest point ([`Run::advance`]), so noise
//!   on a fade can't keep a fall from being counted.
//! - **The window** is 0.98 s: the last 20 ms of the second are a tolerance for the controller's
//!   clock ([`WINDOW_US`]).
//! - **The first frame** has no luminance to change from; nothing is counted for it.

use std::collections::VecDeque;

use folo_provider::ElementColors;

use crate::model::LUMA;

/// A run's cumulative change that makes a transition.
const MIN_CHANGE: f32 = 0.10;
/// A run ends only when the luminance turns back by at least this much.
const REVERSAL: f32 = 0.02;
/// A transition counts only when its darker end is below this.
const DARK_BELOW: f32 = 0.80;
/// How long a flash stays in the window, in microseconds: 1 s less 20 ms, a tolerance for the
/// controller's clock, so a steady 3 Hz blink isn't held by its own fourth flash.
const WINDOW_US: u64 = 980_000;
/// The most flashes the window holds; a transition that would complete one more is held.
const MAX_FLASHES: usize = 3;

/// A series of steps in which the luminance moves one way, give or take noise.
#[derive(Clone, Copy, Debug, Default)]
struct Run {
    /// `1` rising, `-1` falling, `0` before it has moved.
    dir: i8,
    /// The luminance the run's change is measured from.
    start: f32,
    /// The run's furthest point in its direction.
    extreme: f32,
}

impl Run {
    /// Moves the run on to luminance `to`, which follows `from` (what the element shows), and
    /// says in which direction it completes a transition, if it does. The run ends only when the
    /// luminance turns back by at least [`REVERSAL`] from its furthest point, and the next run
    /// is measured from that point.
    fn advance(&mut self, from: f32, to: f32) -> Option<i8> {
        if self.dir == 0 {
            // Exact on purpose: a step that doesn't change the luminance doesn't start a run.
            #[allow(clippy::float_cmp)]
            if to == from {
                return None;
            }
            self.dir = if to > from { 1 } else { -1 };
            self.start = from;
            self.extreme = to;
        } else {
            let sign = f32::from(self.dir);
            if (to - self.extreme) * sign > 0.0 {
                self.extreme = to;
            } else if (self.extreme - to) * sign >= REVERSAL {
                self.start = self.extreme;
                self.dir = -self.dir;
                self.extreme = to;
            }
        }
        let change = (self.extreme - self.start).abs();
        let darker = self.start.min(self.extreme);
        (change >= MIN_CHANGE && darker < DARK_BELOW).then_some(self.dir)
    }

    /// Starts the change again from `luminance`, after a transition.
    fn restart(&mut self, luminance: f32) {
        self.start = luminance;
        self.extreme = luminance;
    }
}

/// One element's state.
#[derive(Clone, Debug, Default)]
struct Element {
    /// Whether a frame has come yet.
    started: bool,
    /// The luminance of what the element shows.
    shown: f32,
    run: Run,
    /// The direction of the last counted transition, `0` before any.
    last_dir: i8,
    /// A flash is open: its first transition is counted, its second hasn't come.
    open: bool,
    /// The times of the flashes in the window, oldest first.
    flashes: VecDeque<u64>,
    /// Whether the last step held the element.
    held: bool,
}

impl Element {
    /// Whether `now_us` leaves room for one more flash in the window.
    fn blocked(&mut self, now_us: u64) -> bool {
        while self
            .flashes
            .front()
            .is_some_and(|t| now_us.saturating_sub(*t) >= WINDOW_US)
        {
            self.flashes.pop_front();
        }
        self.flashes.len() >= MAX_FLASHES
    }

    /// Records a counted transition in direction `dir` at `now_us`.
    fn count(&mut self, dir: i8, now_us: u64) {
        if self.last_dir == 0 {
            self.open = true;
        } else if dir != self.last_dir {
            if self.open {
                self.flashes.push_back(now_us);
                self.open = false;
            } else {
                self.open = true;
            }
        }
        self.last_dir = dir;
    }

    /// Takes the luminance the input would show, and says whether the element is held at what
    /// it shows now instead.
    fn step(&mut self, now_us: u64, luminance: f32) -> bool {
        if !self.started {
            self.started = true;
            self.shown = luminance;
            return false;
        }
        let blocked = self.blocked(now_us);
        // Judged from what the element shows, so a held element is judged the same way on
        // every step until the window lets it go.
        let mut run = self.run;
        let transition = run.advance(self.shown, luminance);
        if let Some(dir) = transition {
            if blocked && self.last_dir == -dir && self.open {
                self.held = true;
                return true;
            }
            self.count(dir, now_us);
            run.restart(luminance);
        }
        self.run = run;
        self.shown = luminance;
        self.held = false;
        false
    }
}

/// The limiter of one target.
pub(crate) struct FlashLimiter {
    /// Wire channel `i` carries logical colour channel `order[i]`.
    order: [usize; 3],
    elements: Vec<Element>,
    /// What the target showed after the last step, to hold elements at.
    shown: Option<ElementColors>,
}

impl FlashLimiter {
    pub(crate) fn new(order: [usize; 3]) -> Self {
        Self {
            order,
            elements: Vec::new(),
            shown: None,
        }
    }

    /// Takes over `from`'s history: each element's run, transitions and flashes in the window,
    /// and what the target showed. A limiter made for new settings of the same lights carries on
    /// where the old one was, so a change of settings can't open a fresh window. The colour
    /// order stays this limiter's own.
    pub(crate) fn inherit(&mut self, from: Self) {
        self.elements = from.elements;
        self.shown = from.shown;
    }

    /// Moves the history out, leaving this limiter limiting from an empty one.
    pub(crate) fn take(&mut self) -> Self {
        std::mem::replace(self, Self::new(self.order))
    }

    /// The luminance of each element, 0–1 (see the table in the architecture document).
    fn luminances(&self, colors: &ElementColors) -> Vec<f32> {
        let rgb = |wire: [f32; 3]| {
            let mut logical = [0.0_f32; 3];
            for (value, at) in wire.iter().zip(self.order) {
                logical[at] = *value;
            }
            (LUMA[0] * logical[0] + LUMA[1] * logical[1] + LUMA[2] * logical[2]) / 255.0
        };
        let l = match colors {
            ElementColors::Rgb(v) => v.iter().map(|c| rgb(*c)).collect(),
            ElementColors::Rgbw(v) => v
                .iter()
                .map(|c| rgb([c[0], c[1], c[2]]) + c[3] / 255.0)
                .collect(),
            ElementColors::RgbCct(v) => v.iter().map(|c| rgb(c.rgb) + c.white / 255.0).collect(),
            ElementColors::Cct(v) => v.iter().map(|c| c.brightness / 255.0).collect(),
            ElementColors::Dimmer(v) => v.iter().map(|c| c / 255.0).collect(),
            ElementColors::Xy(v) => v.iter().map(|c| c.brightness / 255.0).collect(),
            ElementColors::OnOff(v) => v.iter().map(|on| f32::from(u8::from(*on))).collect(),
        };
        let mut l: Vec<f32> = l;
        for value in &mut l {
            *value = if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 1.0)
            };
        }
        l
    }

    /// Limits `colors` in place: an element that is held keeps what it showed, whole.
    pub(crate) fn apply(&mut self, now_us: u64, colors: &mut ElementColors) {
        let luminance = self.luminances(colors);
        if self.elements.len() != luminance.len() {
            self.elements = vec![Element::default(); luminance.len()];
            self.shown = None;
        }
        for (i, (element, l)) in self.elements.iter_mut().zip(luminance).enumerate() {
            if element.step(now_us, l)
                && let Some(shown) = &self.shown
            {
                hold(colors, shown, i);
            }
        }
        self.shown = Some(colors.clone());
    }

    #[cfg(test)]
    fn held(&self) -> Vec<bool> {
        self.elements.iter().map(|e| e.held).collect()
    }
}

/// Puts element `i` of `from` into `colors`.
fn hold(colors: &mut ElementColors, from: &ElementColors, i: usize) {
    match (colors, from) {
        (ElementColors::Rgb(a), ElementColors::Rgb(b)) => a[i] = b[i],
        (ElementColors::Rgbw(a), ElementColors::Rgbw(b)) => a[i] = b[i],
        (ElementColors::RgbCct(a), ElementColors::RgbCct(b)) => a[i] = b[i],
        (ElementColors::Cct(a), ElementColors::Cct(b)) => a[i] = b[i],
        (ElementColors::Dimmer(a), ElementColors::Dimmer(b)) => a[i] = b[i],
        (ElementColors::OnOff(a), ElementColors::OnOff(b)) => a[i] = b[i],
        (ElementColors::Xy(a), ElementColors::Xy(b)) => a[i] = b[i],
        _ => {}
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
    use folo_provider::RgbCct;

    /// A frame step of 60 Hz, in microseconds.
    const STEP: u64 = 16_667;

    fn dimmer(level: f32) -> ElementColors {
        ElementColors::Dimmer(vec![level * 255.0])
    }

    fn level(colors: &ElementColors) -> f32 {
        match colors {
            ElementColors::Dimmer(v) => v[0] / 255.0,
            _ => unreachable!(),
        }
    }

    /// Runs a blink of `half` steps per half-cycle for `seconds`, starting dark, and gives the
    /// (input, output) levels per step.
    fn blink(limiter: &mut FlashLimiter, half: u64, seconds: u64) -> Vec<(f32, f32)> {
        let steps = seconds * 60;
        (0..steps)
            .map(|i| {
                let input = if (i / half) % 2 == 1 { 1.0 } else { 0.0 };
                let mut colors = dimmer(input);
                limiter.apply(i * STEP, &mut colors);
                (input, level(&colors))
            })
            .collect()
    }

    /// The most edges (the output crossing 0.5) in any 1 s. Three flashes are six edges, and a
    /// flash that is open when the window ends adds a seventh.
    fn most_edges_in_a_second(run: &[(f32, f32)]) -> usize {
        let edges: Vec<usize> = run
            .windows(2)
            .enumerate()
            .filter(|(_, w)| (w[0].1 > 0.5) != (w[1].1 > 0.5))
            .map(|(i, _)| i + 1)
            .collect();
        edges
            .iter()
            .map(|a| {
                edges
                    .iter()
                    .filter(|b| **b >= *a && (**b - *a) as u64 * STEP < 1_000_000)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn three_flashes_a_second_pass_untouched() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        // 10 steps of 16.667 ms per half cycle: 3.0 Hz.
        let run = blink(&mut limiter, 10, 6);
        assert!(run.iter().all(|(i, o)| i == o), "3 Hz was altered");
        assert_eq!(most_edges_in_a_second(&run), 6, "the pattern isn't 3 Hz");
    }

    #[test]
    fn four_flashes_a_second_and_more_are_held() {
        for half in [7, 5, 3, 2, 1] {
            let mut limiter = FlashLimiter::new([0, 1, 2]);
            let run = blink(&mut limiter, half, 6);
            assert!(
                run.iter().any(|(i, o)| i != o),
                "nothing was held at {half} steps per half cycle"
            );
            assert!(
                most_edges_in_a_second(&run) <= 7,
                "{} edges shown in a second at {half} steps per half cycle",
                most_edges_in_a_second(&run)
            );
        }
    }

    /// 10 ms steps: `(ms, level)` points, each level held until the next.
    fn timeline(limiter: &mut FlashLimiter, points: &[(u64, f32)], until_ms: u64) -> Vec<f32> {
        (0..=until_ms / 10)
            .map(|i| {
                let ms = i * 10;
                let input = points
                    .iter()
                    .rev()
                    .find(|(t, _)| *t <= ms)
                    .map_or(0.0, |(_, v)| *v);
                let mut colors = dimmer(input);
                limiter.apply(ms * 1000, &mut colors);
                level(&colors)
            })
            .collect()
    }

    const BURST: [(u64, f32); 9] = [
        (0, 0.0),
        (100, 1.0),
        (150, 0.0), // flash 1 completes
        (200, 1.0),
        (250, 0.0), // flash 2
        (300, 1.0),
        (350, 0.0), // flash 3
        (400, 1.0), // opens flash 4
        (450, 0.0), // would complete it
    ];

    #[test]
    fn the_window_slides_and_the_held_element_jumps_to_the_input() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let out = timeline(&mut limiter, &BURST, 1300);
        let at = |ms: usize| out[ms / 10];
        assert_eq!(at(440), 1.0, "the opener passed");
        assert_eq!(
            at(450),
            1.0,
            "the fourth flash is held, the output stays bright"
        );
        assert_eq!(at(1000), 1.0);
        assert_eq!(
            at(1120),
            1.0,
            "the first flash (at 150 ms) is still in the window"
        );
        assert_eq!(
            at(1130),
            0.0,
            "it left the window: the output jumps to the input"
        );
        assert_eq!(at(1300), 0.0);
    }

    #[test]
    fn the_jump_counts_as_the_flash_it_completes() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        timeline(&mut limiter, &BURST, 1130);
        let e = &limiter.elements[0];
        // Flashes at 250, 350 and the jump at 1130; the one at 150 has gone.
        assert_eq!(
            e.flashes.iter().copied().collect::<Vec<_>>(),
            [250_000, 350_000, 1_130_000]
        );
    }

    #[test]
    fn nothing_is_held_when_the_input_has_come_back_by_the_release() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut points = BURST.to_vec();
        points.push((1000, 1.0)); // back where the output is, before the window opens
        let out = timeline(&mut limiter, &points, 1300);
        assert_eq!(out[115], 1.0);
        assert_eq!(out[130], 1.0);
        let e = &limiter.elements[0];
        assert!(!e.held);
        assert!(
            e.flashes.iter().all(|t| *t < 400_000),
            "no transition was counted for the release"
        );
    }

    #[test]
    fn on_off_targets_hold_their_state() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut shown = Vec::new();
        for i in 0..100u64 {
            let ms = i * 10;
            let on = BURST
                .iter()
                .rev()
                .find(|(t, _)| *t <= ms)
                .is_some_and(|(_, v)| *v > 0.5);
            let mut colors = ElementColors::OnOff(vec![on]);
            limiter.apply(ms * 1000, &mut colors);
            let ElementColors::OnOff(v) = colors else {
                unreachable!()
            };
            shown.push(v[0]);
        }
        assert!(shown[44], "on at 440 ms");
        assert!(shown[45], "held on at 450 ms, though the input is off");
        assert!(shown[99], "still held at 990 ms");
        assert_eq!(limiter.held(), [true]);
    }

    #[test]
    fn a_held_element_keeps_its_whole_colour_and_the_white_channel_counts() {
        // A white flash on an RGBW strip: accurate extraction leaves W alone.
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut last_passed = None;
        let mut held_frames = 0;
        for i in 0..120u64 {
            let bright = (i / 4) % 2 == 1; // 7.5 Hz
            // Another channel wiggles by a tiny amount that changes no luminance transition.
            let wiggle = (i % 3) as f32;
            let frame = if bright {
                [wiggle, 0.0, 0.0, 255.0]
            } else {
                [wiggle, 0.0, 0.0, 0.0]
            };
            let mut colors = ElementColors::Rgbw(vec![frame]);
            limiter.apply(i * STEP, &mut colors);
            let ElementColors::Rgbw(v) = &colors else {
                unreachable!()
            };
            if v[0] == frame {
                last_passed = Some(frame);
            } else {
                held_frames += 1;
                assert_eq!(
                    Some(v[0]),
                    last_passed,
                    "held at the last colour that passed, whole"
                );
            }
        }
        assert!(held_frames > 0, "the white flashes weren't limited");
    }

    #[test]
    fn rgbcct_counts_the_cct_channel() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut changed = false;
        for i in 0..120u64 {
            let white = if (i / 4) % 2 == 1 { 255.0 } else { 0.0 };
            let frame = RgbCct {
                rgb: [0.0; 3],
                white,
                kelvin: 4000.0,
            };
            let mut colors = ElementColors::RgbCct(vec![frame]);
            limiter.apply(i * STEP, &mut colors);
            let ElementColors::RgbCct(v) = &colors else {
                unreachable!()
            };
            changed |= v[0] != frame;
        }
        assert!(changed);
    }

    #[test]
    fn luminance_undoes_the_colour_order() {
        // Wire order GRB: wire channel 0 carries green.
        let limiter = FlashLimiter::new([1, 0, 2]);
        let l = limiter.luminances(&ElementColors::Rgb(vec![[255.0, 0.0, 0.0]]));
        assert!((l[0] - LUMA[1]).abs() < 1e-6, "{l:?}");
        let l = limiter.luminances(&ElementColors::Rgb(vec![[0.0, 255.0, 0.0]]));
        assert!((l[0] - LUMA[0]).abs() < 1e-6);
    }

    #[test]
    fn rgbw_luminance_adds_the_white_channel_up_to_one() {
        let limiter = FlashLimiter::new([0, 1, 2]);
        let l = limiter.luminances(&ElementColors::Rgbw(vec![
            [0.0, 0.0, 0.0, 255.0],
            [255.0; 4],
        ]));
        assert_eq!(l, [1.0, 1.0]);
    }

    #[test]
    fn changes_below_a_tenth_are_not_transitions() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut seen = Vec::new();
        for i in 0..240u64 {
            let input = if i % 2 == 0 { 0.30 } else { 0.39 };
            let mut colors = dimmer(input);
            limiter.apply(i * STEP, &mut colors);
            seen.push((input, level(&colors)));
        }
        assert!(seen.iter().all(|(i, o)| (i - o).abs() < 1e-6));
    }

    #[test]
    fn transitions_between_bright_levels_are_not_counted() {
        // Darker end 0.85 is at or above 0.80.
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        for i in 0..240u64 {
            let input = if i % 2 == 0 { 0.85 } else { 1.0 };
            let mut colors = dimmer(input);
            limiter.apply(i * STEP, &mut colors);
            assert!((level(&colors) - input).abs() < 1e-6);
        }
    }

    #[test]
    fn a_slow_ramp_and_a_slow_swing_pass() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        for i in 0..600u64 {
            // 0.5 Hz sine between 0 and 1.
            let t = (i * STEP) as f32 / 1e6;
            let input = 0.5 + 0.5 * (std::f32::consts::TAU * 0.5 * t).sin();
            let mut colors = dimmer(input);
            limiter.apply(i * STEP, &mut colors);
            assert!((level(&colors) - input).abs() < 1e-6, "step {i}");
        }
    }

    #[test]
    fn a_fast_swing_is_limited_even_when_it_is_smooth() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut run = Vec::new();
        for i in 0..600u64 {
            let t = (i * STEP) as f32 / 1e6;
            let input = 0.5 + 0.5 * (std::f32::consts::TAU * 6.0 * t).sin();
            let mut colors = dimmer(input);
            limiter.apply(i * STEP, &mut colors);
            run.push((input, level(&colors)));
        }
        assert!(run.iter().any(|(i, o)| (i - o).abs() > 0.1));
    }

    #[test]
    fn an_inherited_history_keeps_holding() {
        let mut old = FlashLimiter::new([0, 1, 2]);
        let mut now = 0;
        let mut edges = 0;
        let mut last = None;
        // 10 Hz: three steps white, three black; the limiter is replaced halfway
        for i in 0..60u64 {
            if i == 30 {
                let mut fresh = FlashLimiter::new([0, 1, 2]);
                fresh.inherit(old.take());
                old = fresh;
            }
            let v = if (i / 3) % 2 == 0 { 255.0 } else { 0.0 };
            let mut colors = ElementColors::Rgb(vec![[v; 3]]);
            old.apply(now, &mut colors);
            let shown = colors.to_bytes()[0];
            edges += usize::from(last.is_some_and(|l| l != shown));
            last = Some(shown);
            now += STEP;
        }
        assert!(edges <= 7, "{edges} edges in a second");
    }

    #[test]
    fn elements_are_limited_independently() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut second_altered = false;
        for i in 0..120u64 {
            let fast = if (i / 2) % 2 == 1 { 255.0 } else { 0.0 };
            let steady = if (i / 30) % 2 == 1 { 255.0 } else { 0.0 };
            let mut colors = ElementColors::Dimmer(vec![fast, steady]);
            limiter.apply(i * STEP, &mut colors);
            let ElementColors::Dimmer(v) = colors else {
                unreachable!()
            };
            second_altered |= v[1] != steady;
        }
        assert!(
            !second_altered,
            "a 1 Hz element was held by its neighbour's flashing"
        );
        assert_eq!(limiter.held().len(), 2);
    }

    #[test]
    fn the_first_frame_counts_nothing() {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut colors = dimmer(1.0);
        limiter.apply(0, &mut colors);
        assert_eq!(level(&colors), 1.0);
        assert!(limiter.elements[0].flashes.is_empty());
        assert_eq!(limiter.elements[0].last_dir, 0);
    }

    #[test]
    fn a_staircase_with_a_wiggle_is_still_limited() {
        // 6 Hz: a jump to 1.0, then falls of 0.09 each followed by a rise of 0.002 (the review's
        // probe). Each rise is a reversal under 0.02, so the fall stays one run.
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        let mut level_now = 1.0_f32;
        let mut altered = 0;
        for i in 0..600u64 {
            match i % 10 {
                0 => level_now = 1.0,
                n if n % 2 == 1 => level_now -= 0.09,
                _ => level_now += 0.002,
            }
            let mut colors = dimmer(level_now);
            limiter.apply(i * STEP, &mut colors);
            altered += usize::from((level(&colors) - level_now).abs() > 1e-4);
        }
        assert!(altered > 0, "a 6 Hz swing with noise passed untouched");
    }

    #[test]
    fn a_reversal_of_two_hundredths_or_more_ends_a_run() {
        let mut run = Run::default();
        // Falls to 0.5, rises 0.03 (a reversal), falls again.
        assert_eq!(run.advance(1.0, 0.7), Some(-1));
        run.restart(0.7);
        assert_eq!(run.advance(0.7, 0.5), Some(-1));
        run.restart(0.5);
        run.advance(0.5, 0.53);
        assert_eq!(run.dir, 1, "0.03 back is a new run");
        assert_eq!(run.start, 0.5);
    }

    /// A blink between two levels at 10 Hz on the 60 Hz step; whether anything was held.
    fn blink_between(low: f32, high: f32) -> bool {
        let mut limiter = FlashLimiter::new([0, 1, 2]);
        (0..360u64).any(|i| {
            let input = if (i / 3) % 2 == 1 { high } else { low };
            let mut colors = dimmer(input);
            limiter.apply(i * STEP, &mut colors);
            (level(&colors) - input).abs() > 1e-4
        })
    }

    #[test]
    fn the_thresholds_are_pinned_from_both_sides() {
        assert!(blink_between(0.30, 0.41), "a swing of 0.11 is a transition");
        assert!(!blink_between(0.30, 0.39), "a swing of 0.09 isn't");
        assert!(blink_between(0.75, 0.95), "darker end 0.75 is below 0.80");
        assert!(!blink_between(0.82, 1.0), "darker end 0.82 isn't");
    }

    #[test]
    fn a_steady_three_hertz_blink_is_never_held_on_an_exact_clock() {
        for hz in [30u64, 60, 144, 240] {
            let half = hz / 6;
            let mut limiter = FlashLimiter::new([0, 1, 2]);
            for i in 0..(hz * 8) {
                let input = if (i / half) % 2 == 1 { 1.0 } else { 0.0 };
                let mut colors = dimmer(input);
                limiter.apply(i * 1_000_000 / hz, &mut colors);
                assert_eq!(level(&colors), input, "held at {hz} Hz, step {i}");
            }
        }
    }

    #[test]
    fn four_hertz_is_held_on_exact_clocks_too() {
        for hz in [60u64, 144] {
            let mut limiter = FlashLimiter::new([0, 1, 2]);
            let held = (0..hz * 4).any(|i| {
                let input = if (i * 8 / hz) % 2 == 1 { 1.0 } else { 0.0 };
                let mut colors = dimmer(input);
                limiter.apply(i * 1_000_000 / hz, &mut colors);
                level(&colors) != input
            });
            assert!(held, "4 Hz passed at {hz} Hz");
        }
    }
}
