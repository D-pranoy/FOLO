//! Per surface: which of its sources to show (pipeline: controller stages 1
//! and 2). Every call is given the time, so the core steps deterministically.
//!
//! - **Newest frame wins:** a source's frame replaces its last one, unless its
//!   sequence number is not newer and the last one is still fresh.
//! - **Staleness:** a frame older than [`STALE_US`] counts for nothing, and a
//!   source without a fresh frame is out.
//! - **Active:** a source is active when it reported audio activity within the
//!   last [`ACTIVITY_HOLD_US`].
//! - **Winner:** the highest-priority active source; with none active, the
//!   highest-priority source with a fresh frame (so silence still shows the idle
//!   glow); with none fresh, the surface's `background × master`.
//! - **Crossfade:** each source has a weight that moves to 1 (the winner) or 0
//!   (the others) in [`CROSSFADE_US`], and the frame is the weights' average.
//!   With `blend_sources` every active source is a winner and they blend by
//!   per-channel maximum, the surfaces' own default blend.
//!
//! The frames' `master` is already in them, so the compositor never applies it,
//! except to the background.

use std::collections::HashMap;

use folo_protocol::frames::SurfaceFrame;
use folo_protocol::generated::Surface;

use crate::ambient::parse_hex;

/// A source is active for this long after it last reported activity (µs).
pub const ACTIVITY_HOLD_US: u64 = 2_000_000;
/// A frame older than this counts as no frame (µs). Fixed: no document carries a per-surface value.
pub const STALE_US: u64 = 500_000;
/// The time a change of winner takes to fade (µs).
pub const CROSSFADE_US: u64 = 300_000;
/// A frame numbered below this, against a held one numbered [`RESTART_AFTER`] or more,
/// is a source that started over.
const RESTART_SEQUENCE_MAX: u32 = 8;
const RESTART_AFTER: u32 = 1_024;

/// What the compositor needs of a surface.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceSettings {
    /// The surface's id.
    pub id: String,
    /// Source ids, highest priority first.
    pub sources: Vec<String>,
    /// Blend every active source instead of showing the highest-priority one.
    pub blend_sources: bool,
    /// The colour shown when no source has a fresh frame.
    pub background: [u8; 3],
    /// The output scale (0–1), applied to the background; the sources' frames have it already.
    /// The zone's ambient light needs the same value ([`Compositor::master`] gives it back).
    pub master: f64,
    /// Cells across, as the frames arrive (the resolution the controller asked the engine for).
    pub width: u16,
    /// Cells down.
    pub height: u16,
}

impl SurfaceSettings {
    /// Reads a zone's surface. `width` and `height` are the resolution the
    /// frames are published at, which a large canvas has smaller than its own.
    #[must_use]
    pub fn from_surface(surface: &Surface, width: u16, height: u16) -> Self {
        Self {
            id: surface.id.as_str().to_owned(),
            sources: surface
                .sources
                .iter()
                .map(|source| source.as_str().to_owned())
                .collect(),
            blend_sources: surface.blend_sources,
            background: parse_hex(&surface.background),
            master: surface.master,
            width,
            height,
        }
    }

    fn values(&self) -> usize {
        usize::from(self.width) * usize::from(self.height) * 3
    }
}

/// What came of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Kept as the source's newest frame.
    Accepted,
    /// Dropped: its sequence number is the last one's or older, and that frame is still fresh.
    Superseded,
}

/// Why a frame wasn't taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameRefused {
    /// No such surface.
    UnknownSurface,
    /// The surface doesn't list the source.
    UnknownSource,
    /// The frame holds other than `width × height × 3` values (a hand-built one;
    /// decoded frames can't).
    WrongPixelCount {
        /// What the surface's size calls for.
        expected: usize,
        /// What the frame holds.
        actual: usize,
    },
    /// The frame isn't the surface's resolution.
    WrongSize {
        /// The surface's width and height.
        expected: (u16, u16),
        /// The frame's.
        actual: (u16, u16),
    },
}

/// A surface's composited frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Composed {
    /// Cells across.
    pub width: u16,
    /// Cells down.
    pub height: u16,
    /// Red, green and blue of every cell, row-major, on the 0–255 scale.
    pub pixels: Vec<f32>,
    /// The source that wins, or none when the background is shown.
    pub winner: Option<String>,
    /// Several sources are mixed: a crossfade or a blend is under way.
    pub mixing: bool,
}

struct Held {
    pixels: Vec<f32>,
    sequence: u32,
    received_us: u64,
}

struct Slot {
    source: String,
    held: Option<Held>,
    weight: f64,
}

impl Slot {
    fn fresh(&self, now_us: u64) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| now_us.saturating_sub(held.received_us) <= STALE_US)
    }
}

struct SurfaceState {
    settings: SurfaceSettings,
    slots: Vec<Slot>,
    last_us: Option<u64>,
}

/// What a source last said about its audio.
struct Report {
    active: bool,
    reported_us: u64,
    last_active_us: Option<u64>,
}

/// The compositor of a node's surfaces.
#[derive(Default)]
pub struct Compositor {
    surfaces: HashMap<String, SurfaceState>,
    /// By source and surface; `None` is the source as a whole.
    reports: HashMap<(String, Option<String>), Report>,
}

impl Compositor {
    /// An empty compositor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the surfaces. A surface that stays keeps the frames of the
    /// sources that stay (unless its resolution changed) and its fades.
    pub fn configure(&mut self, surfaces: Vec<SurfaceSettings>) {
        let mut old = std::mem::take(&mut self.surfaces);
        for settings in surfaces {
            let before = old.remove(&settings.id);
            let resized = before.as_ref().is_some_and(|b| {
                (b.settings.width, b.settings.height) != (settings.width, settings.height)
            });
            let (mut kept, last_us) =
                before.map_or((Vec::new(), None), |before| (before.slots, before.last_us));
            let slots = settings
                .sources
                .iter()
                .map(|source| {
                    kept.iter()
                        .position(|slot| &slot.source == source)
                        .map(|at| kept.swap_remove(at))
                        .filter(|_| !resized)
                        .unwrap_or_else(|| Slot {
                            source: source.clone(),
                            held: None,
                            weight: 0.0,
                        })
                })
                .collect();
            self.surfaces.insert(
                settings.id.clone(),
                SurfaceState {
                    settings,
                    slots,
                    last_us,
                },
            );
        }
        // reports for surfaces that are gone
        let surfaces = &self.surfaces;
        self.reports
            .retain(|(_, surface), _| surface.as_ref().is_none_or(|id| surfaces.contains_key(id)));
    }

    /// The surface's `master`, for the zone's ambient light, which takes the
    /// same value ([`ZoneAmbient::apply`](crate::ZoneAmbient::apply)).
    #[must_use]
    pub fn master(&self, surface: &str) -> Option<f64> {
        self.surfaces
            .get(surface)
            .map(|state| state.settings.master)
    }

    /// Takes a source's frame for a surface. `now_us` is when it arrived, on
    /// the controller's clock (the frame's own timestamp is the source's).
    ///
    /// # Errors
    /// [`FrameRefused`] when the surface or the source isn't known, or the
    /// frame isn't the surface's resolution and size.
    pub fn frame(
        &mut self,
        surface: &str,
        source: &str,
        frame: &SurfaceFrame,
        now_us: u64,
    ) -> Result<FrameOutcome, FrameRefused> {
        let state = self
            .surfaces
            .get_mut(surface)
            .ok_or(FrameRefused::UnknownSurface)?;
        let expected = (state.settings.width, state.settings.height);
        if (frame.width, frame.height) != expected {
            return Err(FrameRefused::WrongSize {
                expected,
                actual: (frame.width, frame.height),
            });
        }
        if frame.pixels.len() != state.settings.values() {
            return Err(FrameRefused::WrongPixelCount {
                expected: state.settings.values(),
                actual: frame.pixels.len(),
            });
        }
        let slot = state
            .slots
            .iter_mut()
            .find(|slot| slot.source == source)
            .ok_or(FrameRefused::UnknownSource)?;
        if let Some(held) = &slot.held {
            // newer by serial number arithmetic: within half the range ahead
            let ahead = frame.sequence.wrapping_sub(held.sequence);
            let newer = ahead != 0 && ahead < 1 << 31;
            // a restarted source counts from 0 again (the spec keeps the count across
            // reconnects, not across restarts): its first frames, at the very start of
            // the count while the held one is far into it, are taken
            let restarted = frame.sequence < RESTART_SEQUENCE_MAX && held.sequence >= RESTART_AFTER;
            // and once the last frame is stale, any sequence number is
            if !newer && !restarted && now_us.saturating_sub(held.received_us) <= STALE_US {
                return Ok(FrameOutcome::Superseded);
            }
        }
        let mut pixels = frame.to_f32();
        for value in &mut pixels {
            // NaN becomes 0
            *value = if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 255.0)
            };
        }
        slot.held = Some(Held {
            pixels,
            sequence: frame.sequence,
            received_us: now_us,
        });
        Ok(FrameOutcome::Accepted)
    }

    /// Takes an `activity` report. `surface` is the notification's, `None` for
    /// the whole source. `now_us` is when it arrived.
    /// A report for a surface that isn't configured is ignored (reports repeat 10 times
    /// a second, and this keeps a source from growing the table with made-up names).
    pub fn activity(&mut self, source: &str, surface: Option<&str>, active: bool, now_us: u64) {
        if surface.is_some_and(|surface| !self.surfaces.contains_key(surface)) {
            return;
        }
        let report = self
            .reports
            .entry((source.to_owned(), surface.map(ToOwned::to_owned)))
            .or_insert(Report {
                active: false,
                reported_us: now_us,
                last_active_us: None,
            });
        report.active = active;
        report.reported_us = now_us;
        if active {
            report.last_active_us = Some(now_us);
        }
    }

    /// Forgets a source's frames and reports. **The caller (P5b's supervisor) must
    /// call this when a source's link drops or its engine restarts**: until then a
    /// source that starts over at a sequence number the compositor doesn't take
    /// for a restart (anything but a frame 0–7 after 1,024 or more) is dropped
    /// as late for up to [`STALE_US`], and the last frame stays on screen.
    pub fn forget_source(&mut self, source: &str) {
        for state in self.surfaces.values_mut() {
            for slot in state.slots.iter_mut().filter(|s| s.source == source) {
                slot.held = None;
                slot.weight = 0.0;
            }
        }
        self.reports.retain(|(name, _), _| name != source);
    }

    fn report(&self, source: &str, surface: &str) -> Option<&Report> {
        self.reports
            .get(&(source.to_owned(), Some(surface.to_owned())))
            .or_else(|| self.reports.get(&(source.to_owned(), None)))
    }

    /// Whether a source counts as active on a surface: it reported activity
    /// within the last [`ACTIVITY_HOLD_US`].
    #[must_use]
    pub fn is_active(&self, source: &str, surface: &str, now_us: u64) -> bool {
        self.report(source, surface)
            .and_then(|report| report.last_active_us)
            .is_some_and(|at| now_us.saturating_sub(at) <= ACTIVITY_HOLD_US)
    }

    /// What the zone's ambient light counts silence from: some source of the
    /// surface says *now* that its audio is active (its latest report, no older
    /// than [`ACTIVITY_HOLD_US`]), without the hold that [`is_active`](Self::is_active)
    /// adds, so the light keeps the legacy path's timing. A surface with no
    /// source, or none reporting, is quiet.
    #[must_use]
    pub fn is_live(&self, surface: &str, now_us: u64) -> bool {
        self.surfaces.get(surface).is_some_and(|state| {
            state.slots.iter().any(|slot| {
                self.report(&slot.source, surface).is_some_and(|report| {
                    report.active && now_us.saturating_sub(report.reported_us) <= ACTIVITY_HOLD_US
                })
            })
        })
    }

    /// The surface's frame at `now_us`, moving its crossfade on by the time
    /// since its last call. `None` for a surface that isn't configured.
    pub fn compose(&mut self, surface: &str, now_us: u64) -> Option<Composed> {
        // the activity of each source is read before the state is borrowed mutably
        let active: Vec<bool> = {
            let state = self.surfaces.get(surface)?;
            state
                .slots
                .iter()
                .map(|slot| slot.fresh(now_us) && self.is_active(&slot.source, surface, now_us))
                .collect()
        };
        let state = self.surfaces.get_mut(surface)?;
        let dt_us = state.last_us.map_or(0, |last| now_us.saturating_sub(last));
        state.last_us = Some(now_us);
        let settings = &state.settings;
        let fresh: Vec<bool> = state.slots.iter().map(|slot| slot.fresh(now_us)).collect();

        for (slot, &is_fresh) in state.slots.iter_mut().zip(&fresh) {
            if !is_fresh {
                slot.weight = 0.0;
            }
        }
        let first_fresh = fresh.iter().position(|&f| f);
        let Some(first_fresh) = first_fresh else {
            return Some(background(settings));
        };
        let wins: Vec<bool> = if settings.blend_sources && active.iter().any(|&a| a) {
            active.clone()
        } else if let Some(first_active) = active.iter().position(|&a| a) {
            (0..active.len()).map(|i| i == first_active).collect()
        } else {
            (0..active.len()).map(|i| i == first_fresh).collect()
        };

        let anyone_showing = state
            .slots
            .iter()
            .zip(&fresh)
            .any(|(slot, &f)| f && slot.weight > 0.0);
        if anyone_showing {
            // µs to a share of the crossfade: far below 2^53
            #[allow(clippy::cast_precision_loss)]
            let step = dt_us as f64 / CROSSFADE_US as f64;
            for ((slot, &is_fresh), &win) in state.slots.iter_mut().zip(&fresh).zip(&wins) {
                if is_fresh {
                    let target = if win { 1.0 } else { 0.0 };
                    slot.weight += (target - slot.weight).clamp(-step, step);
                }
            }
        } else {
            // nothing to fade from: the winners are shown at once
            for ((slot, &is_fresh), &win) in state.slots.iter_mut().zip(&fresh).zip(&wins) {
                if is_fresh && win {
                    slot.weight = 1.0;
                }
            }
        }

        let showing: Vec<&Slot> = state
            .slots
            .iter()
            .zip(&fresh)
            .filter(|&(slot, &f)| f && slot.weight > 0.0)
            .map(|(slot, _)| slot)
            .collect();
        let winner = wins
            .iter()
            .position(|&w| w)
            .map(|i| state.slots[i].source.clone());
        let pixels = mix(&showing, settings.blend_sources, settings.values());
        Some(Composed {
            width: settings.width,
            height: settings.height,
            pixels,
            winner,
            mixing: showing.len() > 1,
        })
    }
}

/// `background × master`, truncated like the engine's frames are.
fn background(settings: &SurfaceSettings) -> Composed {
    let master = settings.master.clamp(0.0, 1.0);
    // a product of at most 255: truncated to a whole step, so it fits an f32 exactly
    #[allow(clippy::cast_possible_truncation)]
    let cell = settings
        .background
        .map(|c| (f64::from(c) * master).trunc() as f32);
    Composed {
        width: settings.width,
        height: settings.height,
        pixels: cell
            .iter()
            .copied()
            .cycle()
            .take(settings.values())
            .collect(),
        winner: None,
        mixing: false,
    }
}

fn pixels_of(slot: &Slot) -> &[f32] {
    slot.held.as_ref().map_or(&[], |held| &held.pixels)
}

/// The frame of the sources that show. One shows as it is; several average by
/// their weights, or with `blend` take the per-channel maximum of the weighted frames.
fn mix(showing: &[&Slot], blend: bool, values: usize) -> Vec<f32> {
    if let [only] = showing {
        return pixels_of(only).to_vec();
    }
    let mut out = vec![0.0_f32; values];
    if blend {
        for slot in showing {
            // f64 → f32: a factor between 0 and 1
            #[allow(clippy::cast_possible_truncation)]
            let weight = slot.weight as f32;
            for (out, &value) in out.iter_mut().zip(pixels_of(slot)) {
                *out = out.max(value * weight);
            }
        }
    } else {
        let total: f64 = showing.iter().map(|slot| slot.weight).sum();
        let mut sums = vec![0.0_f64; values];
        for slot in showing {
            for (sum, &value) in sums.iter_mut().zip(pixels_of(slot)) {
                *sum += f64::from(value) * slot.weight;
            }
        }
        for (out, sum) in out.iter_mut().zip(sums) {
            // f64 → f32: a mean of values 0–255
            #[allow(clippy::cast_possible_truncation)]
            {
                *out = (sum / total) as f32;
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::float_cmp)]
mod tests {
    use super::*;
    use folo_protocol::frames::Pixels;

    const MS: u64 = 1_000;

    fn settings(sources: &[&str], blend: bool) -> SurfaceSettings {
        SurfaceSettings {
            id: "main".into(),
            sources: sources.iter().map(|s| (*s).to_owned()).collect(),
            blend_sources: blend,
            background: [10, 20, 30],
            master: 0.5,
            width: 2,
            height: 1,
        }
    }

    /// A 2-cell strip of `value` in every channel.
    fn flat(value: u8, sequence: u32) -> SurfaceFrame {
        SurfaceFrame::new(0, sequence, 0, 2, 1, Pixels::Rgb8(vec![value; 6]))
    }

    fn compositor(sources: &[&str], blend: bool) -> Compositor {
        let mut c = Compositor::new();
        c.configure(vec![settings(sources, blend)]);
        c
    }

    fn shown(c: &mut Compositor, now_us: u64) -> Vec<f32> {
        c.compose("main", now_us).unwrap().pixels
    }

    #[test]
    fn with_no_frame_the_background_times_master_is_shown_truncated() {
        let mut c = compositor(&["a"], false);
        let composed = c.compose("main", 0).unwrap();
        assert_eq!(composed.pixels, vec![5.0, 10.0, 15.0, 5.0, 10.0, 15.0]);
        assert_eq!(composed.winner, None);
        assert!(c.compose("nope", 0).is_none());
    }

    #[test]
    fn one_source_shows_as_it_is_even_when_inactive() {
        let mut c = compositor(&["a"], false);
        assert_eq!(
            c.frame("main", "a", &flat(77, 0), 0),
            Ok(FrameOutcome::Accepted)
        );
        let composed = c.compose("main", 10 * MS).unwrap();
        assert_eq!(composed.pixels, vec![77.0; 6]);
        assert_eq!(composed.winner.as_deref(), Some("a"));
        assert!(!composed.mixing);
    }

    #[test]
    fn a_frame_is_stale_after_five_hundred_milliseconds() {
        let mut c = compositor(&["a"], false);
        c.frame("main", "a", &flat(77, 0), 1_000 * MS).unwrap();
        assert_eq!(shown(&mut c, 1_500 * MS), vec![77.0; 6]);
        let composed = c.compose("main", 1_500 * MS + 1).unwrap();
        assert_eq!(composed.pixels, vec![5.0, 10.0, 15.0, 5.0, 10.0, 15.0]);
        assert_eq!(composed.winner, None);
        // a new frame brings it back, at once
        c.frame("main", "a", &flat(80, 1), 1_600 * MS).unwrap();
        assert_eq!(shown(&mut c, 1_600 * MS), vec![80.0; 6]);
    }

    #[test]
    fn the_highest_priority_active_source_wins() {
        let mut c = compositor(&["a", "b"], false);
        c.frame("main", "a", &flat(10, 0), 0).unwrap();
        c.frame("main", "b", &flat(200, 0), 0).unwrap();
        // neither is active: the highest priority with a fresh frame shows
        assert_eq!(c.compose("main", 0).unwrap().winner.as_deref(), Some("a"));
        // only the lower one is: it wins, with a crossfade
        c.activity("b", None, true, 0);
        c.compose("main", 0);
        assert_eq!(
            c.compose("main", 150 * MS).unwrap().winner.as_deref(),
            Some("b")
        );
        // both are: the higher wins again
        c.activity("a", None, true, 150 * MS);
        assert_eq!(
            c.compose("main", 150 * MS).unwrap().winner.as_deref(),
            Some("a")
        );
    }

    #[test]
    fn a_source_whose_activity_lapses_gives_way_after_the_hold_and_a_fade() {
        let mut c = compositor(&["a", "b"], false);
        c.activity("a", None, true, 0);
        // both keep sending and b keeps reporting, every 10 ms; a reported once
        let mut sequence = 0;
        let mut at = |c: &mut Compositor, ms: u64| {
            sequence += 1;
            let now = ms * MS;
            c.frame("main", "a", &flat(100, sequence), now).unwrap();
            c.frame("main", "b", &flat(200, sequence), now).unwrap();
            c.activity("b", None, true, now);
            shown(c, now)[0]
        };
        // a is the higher priority and active (for 2 s), so it shows
        for ms in (0..=2_000).step_by(10) {
            assert_eq!(at(&mut c, ms), 100.0, "at {ms} ms");
        }
        // then b takes over over 300 ms, linearly
        for ms in (2_010..=2_300).step_by(10) {
            let expected = 100.0 + 100.0 * (ms - 2_000) as f32 / 300.0;
            assert!((at(&mut c, ms) - expected).abs() < 1e-3, "at {ms} ms");
        }
        assert_eq!(at(&mut c, 2_310), 200.0);
    }

    #[test]
    fn the_fade_is_linear_and_complete_in_three_hundred_milliseconds() {
        let mut c = compositor(&["a", "b"], false);
        c.frame("main", "a", &flat(100, 0), 0).unwrap();
        c.frame("main", "b", &flat(200, 0), 0).unwrap();
        assert_eq!(shown(&mut c, 0), vec![100.0; 6]);
        c.activity("b", None, true, 0);
        assert_eq!(shown(&mut c, 0), vec![100.0; 6]); // the fade starts from here
        let mid = shown(&mut c, 150 * MS);
        assert!((mid[0] - 150.0).abs() < 1e-3, "{mid:?}");
        let quarter_more = shown(&mut c, 225 * MS);
        assert!((quarter_more[0] - 175.0).abs() < 1e-3, "{quarter_more:?}");
        assert_eq!(shown(&mut c, 300 * MS), vec![200.0; 6]);
        assert_eq!(shown(&mut c, 400 * MS), vec![200.0; 6]);
    }

    #[test]
    fn a_fade_in_progress_turns_around_without_a_jump() {
        let mut c = compositor(&["a", "b"], false);
        c.frame("main", "a", &flat(100, 0), 0).unwrap();
        c.frame("main", "b", &flat(200, 0), 0).unwrap();
        shown(&mut c, 0);
        c.activity("b", None, true, 0);
        shown(&mut c, 0);
        let mid = shown(&mut c, 150 * MS)[0];
        // a becomes the winner again (it's higher priority and now active)
        c.activity("a", None, true, 150 * MS);
        let turned = shown(&mut c, 150 * MS)[0];
        assert!((turned - mid).abs() < 1e-3);
        let back = shown(&mut c, 180 * MS)[0];
        assert!(back < mid && back > 100.0);
    }

    #[test]
    fn activity_holds_for_two_seconds() {
        let mut c = compositor(&["a"], false);
        c.activity("a", None, true, 5_000 * MS);
        c.activity("a", None, false, 5_100 * MS);
        assert!(c.is_active("a", "main", 7_000 * MS));
        assert!(!c.is_active("a", "main", 7_000 * MS + 1));
        assert!(!c.is_active("b", "main", 5_000 * MS));
    }

    #[test]
    fn a_surfaces_own_report_wins_over_its_sources() {
        let mut c = compositor(&["a"], false);
        c.activity("a", None, true, 0);
        c.activity("a", Some("main"), false, 0);
        assert!(!c.is_active("a", "main", 0));
        assert!(c.is_active("a", "other", 0));
    }

    #[test]
    fn the_zones_silence_follows_the_latest_report_without_the_hold() {
        let mut c = compositor(&["a"], false);
        assert!(!c.is_live("main", 0));
        c.activity("a", None, true, 0);
        assert!(c.is_live("main", 100 * MS));
        c.activity("a", None, false, 100 * MS);
        assert!(!c.is_live("main", 100 * MS)); // the 2 s hold is the compositor's, not the light's
        c.activity("a", None, true, 200 * MS);
        assert!(c.is_live("main", 2_200 * MS));
        assert!(!c.is_live("main", 2_200 * MS + 1)); // reports that stop count as quiet
        assert!(!c.is_live("nope", 0));
    }

    #[test]
    fn newest_frame_wins() {
        let mut c = compositor(&["a"], false);
        c.frame("main", "a", &flat(50, 10), 0).unwrap();
        assert_eq!(
            c.frame("main", "a", &flat(60, 10), MS),
            Ok(FrameOutcome::Superseded)
        );
        assert_eq!(
            c.frame("main", "a", &flat(60, 9), MS),
            Ok(FrameOutcome::Superseded)
        );
        assert_eq!(shown(&mut c, 2 * MS), vec![50.0; 6]);
        assert_eq!(
            c.frame("main", "a", &flat(70, 11), 2 * MS),
            Ok(FrameOutcome::Accepted)
        );
        assert_eq!(shown(&mut c, 3 * MS), vec![70.0; 6]);
    }

    #[test]
    fn sequence_numbers_wrap_at_two_to_the_thirty_second() {
        let mut c = compositor(&["a"], false);
        c.frame("main", "a", &flat(50, u32::MAX), 0).unwrap();
        assert_eq!(
            c.frame("main", "a", &flat(60, 0), MS),
            Ok(FrameOutcome::Accepted)
        );
        assert_eq!(
            c.frame("main", "a", &flat(40, u32::MAX), 2 * MS),
            Ok(FrameOutcome::Superseded)
        );
    }

    #[test]
    fn a_source_that_starts_over_is_taken_at_once() {
        let mut c = compositor(&["a"], false);
        c.frame("main", "a", &flat(50, 5_000), 0).unwrap();
        // the first frames of a count that began again, 0 to 7
        assert_eq!(
            c.frame("main", "a", &flat(60, 0), 10 * MS),
            Ok(FrameOutcome::Accepted)
        );
        assert_eq!(
            c.frame("main", "a", &flat(61, 1), 20 * MS),
            Ok(FrameOutcome::Accepted)
        );
        assert_eq!(shown(&mut c, 20 * MS), vec![61.0; 6]);
        // a late frame of the old count is not one: held is near 0 now
        assert_eq!(
            c.frame("main", "a", &flat(1, 0), 30 * MS),
            Ok(FrameOutcome::Superseded)
        );
        // while a small number against a small held one is only late
        assert_eq!(
            c.frame("main", "a", &flat(2, 7), 40 * MS),
            Ok(FrameOutcome::Accepted)
        );
        assert_eq!(
            c.frame("main", "a", &flat(3, 3), 50 * MS),
            Ok(FrameOutcome::Superseded)
        );
    }

    #[test]
    fn other_sequence_numbers_that_go_back_wait_for_the_last_frame_to_go_stale() {
        let mut c = compositor(&["a"], false);
        c.frame("main", "a", &flat(50, 5_000), 0).unwrap();
        assert_eq!(
            c.frame("main", "a", &flat(60, 100), 100 * MS),
            Ok(FrameOutcome::Superseded)
        );
        assert_eq!(
            c.frame("main", "a", &flat(60, 100), 500 * MS),
            Ok(FrameOutcome::Superseded)
        );
        assert_eq!(
            c.frame("main", "a", &flat(60, 100), 500 * MS + 1),
            Ok(FrameOutcome::Accepted)
        );
        // or at once, once its link is forgotten
        c.frame("main", "a", &flat(70, 5_000), 700 * MS).unwrap();
        c.forget_source("a");
        assert_eq!(
            c.frame("main", "a", &flat(80, 100), 710 * MS),
            Ok(FrameOutcome::Accepted)
        );
    }

    #[test]
    fn frames_that_dont_fit_are_refused() {
        let mut c = compositor(&["a"], false);
        assert_eq!(
            c.frame("nope", "a", &flat(1, 0), 0),
            Err(FrameRefused::UnknownSurface)
        );
        assert_eq!(
            c.frame("main", "z", &flat(1, 0), 0),
            Err(FrameRefused::UnknownSource)
        );
        let wide = SurfaceFrame::new(0, 0, 0, 3, 1, Pixels::Rgb8(vec![0; 9]));
        assert_eq!(
            c.frame("main", "a", &wide, 0),
            Err(FrameRefused::WrongSize {
                expected: (2, 1),
                actual: (3, 1)
            })
        );
    }

    #[test]
    fn blending_takes_the_maximum_of_the_active_sources() {
        let mut c = compositor(&["a", "b", "c"], true);
        let frame =
            |values: [u8; 6]| SurfaceFrame::new(0, 0, 0, 2, 1, Pixels::Rgb8(values.to_vec()));
        c.frame("main", "a", &frame([200, 0, 0, 0, 0, 0]), 0)
            .unwrap();
        c.frame("main", "b", &frame([0, 90, 0, 0, 0, 50]), 0)
            .unwrap();
        c.frame("main", "c", &flat(255, 0), 0).unwrap();
        c.activity("a", None, true, 0);
        assert_eq!(shown(&mut c, 0), vec![200.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // b joins over the crossfade; c isn't active, so it stays out
        c.activity("b", None, true, 0);
        assert_eq!(shown(&mut c, 0), vec![200.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            shown(&mut c, 150 * MS),
            vec![200.0, 45.0, 0.0, 0.0, 0.0, 25.0]
        );
        let done = c.compose("main", 300 * MS).unwrap();
        assert_eq!(done.pixels, vec![200.0, 90.0, 0.0, 0.0, 0.0, 50.0]);
        assert!(done.mixing);
        // started together, they blend at once
        let mut together = compositor(&["a", "b"], true);
        together
            .frame("main", "a", &frame([200, 0, 0, 0, 0, 0]), 0)
            .unwrap();
        together
            .frame("main", "b", &frame([0, 90, 0, 0, 0, 50]), 0)
            .unwrap();
        together.activity("a", None, true, 0);
        together.activity("b", None, true, 0);
        assert_eq!(
            shown(&mut together, 0),
            vec![200.0, 90.0, 0.0, 0.0, 0.0, 50.0]
        );
    }

    #[test]
    fn blending_with_none_active_shows_the_first_fresh_source() {
        let mut c = compositor(&["a", "b"], true);
        c.frame("main", "b", &flat(90, 0), 0).unwrap();
        c.frame("main", "a", &flat(30, 0), 0).unwrap();
        assert_eq!(shown(&mut c, 0), vec![30.0; 6]);
    }

    #[test]
    fn stale_sources_drop_out_of_a_fade_at_once() {
        let mut c = compositor(&["a", "b"], false);
        c.frame("main", "a", &flat(100, 0), 0).unwrap();
        c.frame("main", "b", &flat(200, 0), 0).unwrap();
        shown(&mut c, 0);
        c.activity("b", None, true, 0);
        shown(&mut c, 0);
        shown(&mut c, 100 * MS);
        // a goes stale (b keeps sending): only b is left, at full
        c.frame("main", "b", &flat(200, 1), 400 * MS).unwrap();
        c.frame("main", "b", &flat(200, 2), 520 * MS).unwrap();
        assert_eq!(shown(&mut c, 520 * MS), vec![200.0; 6]);
    }

    #[test]
    fn configure_keeps_frames_and_drops_them_on_a_resolution_change() {
        let mut c = compositor(&["a", "b"], false);
        c.frame("main", "a", &flat(100, 0), 0).unwrap();
        c.frame("main", "b", &flat(200, 0), 0).unwrap();
        c.configure(vec![settings(&["b"], false)]);
        assert_eq!(shown(&mut c, MS), vec![200.0; 6]);
        let mut wider = settings(&["b"], false);
        wider.width = 3;
        c.configure(vec![wider]);
        assert_eq!(c.compose("main", 2 * MS).unwrap().winner, None);
        c.configure(vec![]);
        assert!(c.compose("main", 3 * MS).is_none());
    }

    #[test]
    fn out_of_range_and_nan_values_are_clamped() {
        let mut c = compositor(&["a"], false);
        let frame = SurfaceFrame::new(
            0,
            0,
            0,
            2,
            1,
            Pixels::RgbF32(vec![f32::NAN, -5.0, 300.0, 1.5, f32::INFINITY, 0.0]),
        );
        c.frame("main", "a", &frame, 0).unwrap();
        assert_eq!(shown(&mut c, 0), vec![0.0, 0.0, 255.0, 1.5, 255.0, 0.0]);
    }

    #[test]
    fn a_frame_with_the_wrong_number_of_values_is_refused() {
        let mut c = compositor(&["a"], false);
        let short = SurfaceFrame::new(0, 0, 0, 2, 1, Pixels::Rgb8(vec![0; 5]));
        assert_eq!(
            c.frame("main", "a", &short, 0),
            Err(FrameRefused::WrongPixelCount {
                expected: 6,
                actual: 5
            })
        );
        assert_eq!(c.compose("main", 0).unwrap().winner, None);
    }

    #[test]
    fn reports_for_surfaces_that_are_not_configured_are_ignored() {
        let mut c = compositor(&["a"], false);
        c.activity("a", Some("made-up"), true, 0);
        assert!(!c.is_active("a", "made-up", 0));
        c.activity("a", Some("main"), true, 0);
        assert!(c.is_active("a", "main", 0));
        // and a surface that goes takes its reports with it
        c.configure(vec![]);
        c.configure(vec![settings(&["a"], false)]);
        assert!(!c.is_active("a", "main", 0));
    }

    #[test]
    fn the_masters_of_the_surfaces_are_the_documents() {
        let c = compositor(&["a"], false);
        assert_eq!(c.master("main"), Some(0.5));
        assert_eq!(c.master("nope"), None);
    }

    #[test]
    fn surfaces_come_from_the_document() {
        let surface: Surface = serde_json::from_value(serde_json::json!({
            "id": "main",
            "sources": ["local", "ext"],
            "blend_sources": true,
            "background": "#102030",
            "master": 0.25,
        }))
        .unwrap();
        let s = SurfaceSettings::from_surface(&surface, 150, 1);
        assert_eq!(s.id, "main");
        assert_eq!(s.sources, vec!["local", "ext"]);
        assert!(s.blend_sources);
        assert_eq!(s.background, [0x10, 0x20, 0x30]);
        assert_eq!((s.master, s.width, s.height), (0.25, 150, 1));
    }
}
