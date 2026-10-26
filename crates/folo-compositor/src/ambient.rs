//! A zone's reactive mode and ambient light: the controller's version of
//! `source/folo_source/legacy/ambient.py`.
//!
//! The maths is the legacy `Ambient`'s, step for step. What differs is where
//! it runs: the legacy path blends before `master`, on floats. The controller
//! gets frames that already have `master` applied and are truncated to bytes,
//! so the light is `colour × level × master` and is blended into the composited
//! frame, and the result is truncated again (ADR-0036 row 4). A fade of 0
//! leaves the frame untouched. Frames then differ from the legacy ones by at
//! most 1 per channel, which the golden tests check.

use folo_protocol::generated::{self, Zone, ZoneReactive};

/// Seconds to fade into the ambient light.
pub const FADE_IN_S: f64 = 1.5;
/// Seconds to fade back to the reactive look, so the first beat after a pause shows.
pub const FADE_OUT_S: f64 = 0.15;
/// Seconds per breath of the breathe style.
pub const BREATHE_PERIOD_S: f64 = 8.0;
/// Breathe style: the dimmest point, as a share of the colour.
pub const BREATHE_LOW: f64 = 0.3;
/// With "on pause", auto mode switches once the audio has been quiet this long
/// while the player is paused.
pub const PAUSE_QUIET_S: f64 = 0.3;

/// The zone's audio reactiveness (`reactive`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reactive {
    /// The sources' surfaces are shown.
    On,
    /// The sources' surfaces are shown while they are active; the ambient light
    /// after `idle_after` seconds of silence.
    Auto,
    /// The ambient light is shown.
    Off,
}

/// How the ambient light looks (`ambient.style`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmbientStyle {
    /// The colour as it is.
    Solid,
    /// 30–100% of the colour over [`BREATHE_PERIOD_S`].
    Breathe,
}

/// What a zone's document says about its ambient light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientSettings {
    /// `reactive`.
    pub reactive: Reactive,
    /// `ambient.color`.
    pub color: [u8; 3],
    /// `ambient.style`.
    pub style: AmbientStyle,
    /// `ambient.level`, the linear output scale (0–1).
    pub level: f64,
    /// `ambient.idle_after`, seconds of silence before auto mode switches.
    pub idle_after_s: f64,
    /// `ambient.on_pause`: in auto, switch after [`PAUSE_QUIET_S`] when the player is paused.
    pub on_pause: bool,
}

impl Default for AmbientSettings {
    fn default() -> Self {
        Self {
            reactive: Reactive::On,
            color: [0x4B, 0x00, 0xFF],
            style: AmbientStyle::Solid,
            level: 0.35,
            idle_after_s: 5.0,
            on_pause: true,
        }
    }
}

impl AmbientSettings {
    /// Reads a zone's `reactive` and `ambient`.
    #[must_use]
    pub fn from_zone(zone: &Zone) -> Self {
        let ambient = &zone.ambient;
        Self {
            reactive: match zone.reactive {
                ZoneReactive::On => Reactive::On,
                ZoneReactive::Auto => Reactive::Auto,
                ZoneReactive::Off => Reactive::Off,
            },
            color: parse_hex(&ambient.color),
            style: match ambient.style {
                generated::AmbientStyle::Solid => AmbientStyle::Solid,
                generated::AmbientStyle::Breathe => AmbientStyle::Breathe,
            },
            level: ambient.level,
            idle_after_s: ambient.idle_after,
            on_pause: ambient.on_pause,
        }
    }
}

/// `#RRGGBB` as bytes; anything else is black (documents are clamped before they get here).
pub(crate) fn parse_hex(text: &str) -> [u8; 3] {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.is_ascii() {
        return [0; 3];
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
    [byte(0), byte(2), byte(4)]
}

/// One zone's ambient light: crossfades between the reactive frame and the
/// ambient light. [`amount`](Self::amount) is 0 for the reactive look and 1
/// for the ambient light; [`quiet_s`](Self::quiet_s) counts the seconds the
/// audio has been silent.
#[derive(Clone, Debug)]
pub struct ZoneAmbient {
    settings: AmbientSettings,
    amount: f64,
    quiet_s: f64,
    phase: f64,
    last_us: Option<u64>,
}

impl ZoneAmbient {
    /// Starts reactive (amount 0), with no silence counted.
    #[must_use]
    pub fn new(settings: AmbientSettings) -> Self {
        Self {
            settings,
            amount: 0.0,
            quiet_s: 0.0,
            phase: 0.0,
            last_us: None,
        }
    }

    /// Applies new settings. The fade, the silence and the breath carry on,
    /// so a change of colour or mode fades like any other.
    pub fn configure(&mut self, settings: AmbientSettings) {
        self.settings = settings;
    }

    /// The settings in force.
    #[must_use]
    pub fn settings(&self) -> &AmbientSettings {
        &self.settings
    }

    /// The fade: 0 reactive … 1 ambient light (the live channel's `ambient`).
    #[must_use]
    pub fn amount(&self) -> f64 {
        self.amount
    }

    /// Seconds of silence so far (the live channel's `quiet_s`).
    #[must_use]
    pub fn quiet_s(&self) -> f64 {
        self.quiet_s
    }

    /// The share of the ambient light shown: [`amount`](Self::amount) eased.
    #[must_use]
    pub fn eased(&self) -> f64 {
        self.amount * self.amount * (3.0 - 2.0 * self.amount)
    }

    /// 1 for the ambient light, 0 for the reactive look. `paused`: every source
    /// with media says it's paused (it only counts with `on_pause`).
    fn wanted(&self, paused: bool) -> f64 {
        match self.settings.reactive {
            Reactive::Off => 1.0,
            Reactive::Auto => {
                let mut idle = self.settings.idle_after_s;
                if paused && self.settings.on_pause {
                    idle = idle.min(PAUSE_QUIET_S);
                }
                if self.quiet_s >= idle { 1.0 } else { 0.0 }
            }
            Reactive::On => 0.0,
        }
    }

    /// Moves on by `dt_s` seconds. `active`: a source feeding the zone says its
    /// audio is active (none do: silence, so the count runs from the start).
    /// `paused`: every linked source that has media reports it paused.
    pub fn step(&mut self, dt_s: f64, active: bool, paused: bool) {
        self.quiet_s = if active { 0.0 } else { self.quiet_s + dt_s };
        self.phase = (self.phase + dt_s / BREATHE_PERIOD_S) % 1.0;
        let want = self.wanted(paused);
        if want > self.amount {
            self.amount = want.min(self.amount + dt_s / FADE_IN_S);
        } else if want < self.amount {
            self.amount = want.max(self.amount - dt_s / FADE_OUT_S);
        }
    }

    /// [`step`](Self::step) by the time since the last call (0 on the first),
    /// for a core driven by a clock.
    pub fn step_to(&mut self, now_us: u64, active: bool, paused: bool) {
        let dt_us = self.last_us.map_or(0, |last| now_us.saturating_sub(last));
        self.last_us = Some(now_us);
        // µs to s: the counts are far below 2^53
        #[allow(clippy::cast_precision_loss)]
        self.step(dt_us as f64 / 1_000_000.0, active, paused);
    }

    /// The ambient light's colour on the 0–255 scale: the colour × level, ×
    /// the breath when it breathes, × the surface's `master`.
    #[must_use]
    pub fn light(&self, master: f64) -> [f64; 3] {
        let mut scale = self.settings.level * master;
        if self.settings.style == AmbientStyle::Breathe {
            let breath = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * self.phase).cos();
            scale *= BREATHE_LOW + (1.0 - BREATHE_LOW) * breath;
        }
        self.settings.color.map(|c| f64::from(c) * scale)
    }

    /// Mixes the ambient light into `pixels` (cells × rgb, `master` already in
    /// them), by [`eased`](Self::eased), and truncates the result to whole
    /// steps like the legacy path's bytes. At amount 0 `pixels` stays as it is.
    pub fn apply(&self, pixels: &mut [f32], master: f64) {
        if self.amount <= 0.0 {
            return;
        }
        let a = self.eased();
        let light = self.light(master);
        for cell in pixels.as_chunks_mut::<3>().0 {
            for (value, light) in cell.iter_mut().zip(light) {
                let mixed = f64::from(*value).mul_add(1.0 - a, light * a);
                // NaN (a frame that never was one) becomes 0
                let mixed = if mixed.is_nan() {
                    0.0
                } else {
                    mixed.clamp(0.0, 255.0)
                };
                #[allow(clippy::cast_possible_truncation)]
                {
                    *value = mixed.trunc() as f32;
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    fn auto(idle_after_s: f64) -> AmbientSettings {
        AmbientSettings {
            reactive: Reactive::Auto,
            idle_after_s,
            ..AmbientSettings::default()
        }
    }

    /// Steps in 10 ms for `seconds`, with the audio's activity and the pause as given.
    fn run(ambient: &mut ZoneAmbient, seconds: f64, active: bool, paused: bool) {
        let steps = (seconds * 100.0).round() as usize;
        for _ in 0..steps {
            ambient.step(0.01, active, paused);
        }
    }

    #[test]
    fn on_never_leaves_the_reactive_look() {
        let mut ambient = ZoneAmbient::new(AmbientSettings::default());
        run(&mut ambient, 60.0, false, true);
        assert_eq!(ambient.amount(), 0.0);
        assert!((ambient.quiet_s() - 60.0).abs() < 1e-9);
    }

    #[test]
    fn off_fades_in_over_one_and_a_half_seconds() {
        let mut ambient = ZoneAmbient::new(AmbientSettings {
            reactive: Reactive::Off,
            ..AmbientSettings::default()
        });
        run(&mut ambient, 0.75, true, false);
        assert!((ambient.amount() - 0.5).abs() < 1e-9);
        assert!((ambient.eased() - 0.5).abs() < 1e-9);
        run(&mut ambient, 0.75, true, false);
        assert_eq!(ambient.amount(), 1.0);
        run(&mut ambient, 5.0, true, false);
        assert_eq!(ambient.amount(), 1.0);
    }

    #[test]
    fn the_fade_is_eased() {
        let mut ambient = ZoneAmbient::new(AmbientSettings {
            reactive: Reactive::Off,
            ..AmbientSettings::default()
        });
        ambient.step(0.375, true, false); // a quarter of the way
        assert!((ambient.amount() - 0.25).abs() < 1e-9);
        assert!((ambient.eased() - 0.15625).abs() < 1e-9);
    }

    #[test]
    fn auto_waits_for_idle_after_seconds_of_silence() {
        let mut ambient = ZoneAmbient::new(auto(1.0));
        run(&mut ambient, 0.99, false, false);
        assert_eq!(ambient.amount(), 0.0);
        run(&mut ambient, 0.02, false, false);
        assert!(ambient.amount() > 0.0);
        // any sound resets the count
        let mut ambient = ZoneAmbient::new(auto(1.0));
        run(&mut ambient, 0.9, false, false);
        ambient.step(0.01, true, false);
        assert_eq!(ambient.quiet_s(), 0.0);
        run(&mut ambient, 0.9, false, false);
        assert_eq!(ambient.amount(), 0.0);
    }

    #[test]
    fn on_pause_switches_after_a_third_of_a_second_of_silence() {
        let mut ambient = ZoneAmbient::new(auto(30.0));
        run(&mut ambient, 0.29, false, true);
        assert_eq!(ambient.amount(), 0.0);
        run(&mut ambient, 0.03, false, true);
        assert!(ambient.amount() > 0.0);
    }

    #[test]
    fn on_pause_needs_the_setting_and_silence() {
        let mut off = ZoneAmbient::new(AmbientSettings {
            on_pause: false,
            ..auto(30.0)
        });
        run(&mut off, 1.0, false, true);
        assert_eq!(off.amount(), 0.0);
        // other sound (a game) keeps it reactive while the player is paused
        let mut loud = ZoneAmbient::new(auto(30.0));
        run(&mut loud, 5.0, true, true);
        assert_eq!(loud.amount(), 0.0);
        // a longer idle_after isn't shortened to more than itself
        let mut short = ZoneAmbient::new(auto(0.1));
        run(&mut short, 0.12, false, true);
        assert!(short.amount() > 0.0);
    }

    #[test]
    fn fading_out_takes_fifteen_hundredths_of_a_second() {
        let mut ambient = ZoneAmbient::new(auto(1.0));
        run(&mut ambient, 5.0, false, false);
        assert_eq!(ambient.amount(), 1.0);
        ambient.step(0.01, true, false);
        run(&mut ambient, 0.07, true, false);
        assert!((ambient.amount() - (1.0 - 0.08 / FADE_OUT_S)).abs() < 1e-9);
        run(&mut ambient, 0.1, true, false);
        assert_eq!(ambient.amount(), 0.0);
    }

    #[test]
    fn a_zone_with_no_source_counts_quiet_from_the_start() {
        let mut ambient = ZoneAmbient::new(auto(5.0));
        run(&mut ambient, 5.01, false, false);
        assert!(ambient.amount() > 0.0);
    }

    #[test]
    fn breathing_goes_from_thirty_to_a_hundred_percent_over_eight_seconds() {
        let mut ambient = ZoneAmbient::new(AmbientSettings {
            style: AmbientStyle::Breathe,
            level: 1.0,
            color: [200, 100, 50],
            ..AmbientSettings::default()
        });
        let at = |ambient: &ZoneAmbient| ambient.light(1.0)[0];
        assert!((at(&ambient) - 200.0 * 0.3).abs() < 1e-9);
        run(&mut ambient, 4.0, true, false);
        assert!((at(&ambient) - 200.0).abs() < 1e-6);
        run(&mut ambient, 4.0, true, false);
        assert!((at(&ambient) - 60.0).abs() < 1e-6);
    }

    #[test]
    fn the_light_is_colour_times_level_times_master() {
        let ambient = ZoneAmbient::new(AmbientSettings {
            color: [255, 128, 0],
            level: 0.5,
            ..AmbientSettings::default()
        });
        assert_eq!(ambient.light(0.5), [255.0 * 0.25, 128.0 * 0.25, 0.0]);
    }

    #[test]
    fn a_fade_of_zero_leaves_the_frame_untouched() {
        let ambient = ZoneAmbient::new(AmbientSettings::default());
        let mut pixels = vec![1.5, 2.25, 255.0, 0.1, 7.0, 9.9];
        let before = pixels.clone();
        ambient.apply(&mut pixels, 0.7);
        assert_eq!(pixels, before);
    }

    #[test]
    fn the_light_replaces_the_frame_at_full_fade_and_is_truncated() {
        let mut ambient = ZoneAmbient::new(AmbientSettings {
            reactive: Reactive::Off,
            color: [255, 100, 3],
            level: 0.5,
            ..AmbientSettings::default()
        });
        run(&mut ambient, 2.0, true, false);
        let mut pixels = vec![9.0, 200.0, 77.0, 255.0, 255.0, 255.0];
        ambient.apply(&mut pixels, 0.5);
        // 255 × 0.5 × 0.5 = 63.75, 100 × 0.25 = 25, 3 × 0.25 = 0.75
        assert_eq!(pixels, vec![63.0, 25.0, 0.0, 63.0, 25.0, 0.0]);
    }

    #[test]
    fn half_way_mixes_the_frame_and_the_light_by_the_eased_share() {
        let mut ambient = ZoneAmbient::new(AmbientSettings {
            reactive: Reactive::Off,
            color: [200, 200, 200],
            level: 1.0,
            ..AmbientSettings::default()
        });
        ambient.step(0.75, true, false);
        let mut pixels = vec![100.0, 0.0, 255.0];
        ambient.apply(&mut pixels, 1.0);
        assert_eq!(pixels, vec![150.0, 100.0, 227.0]); // ⌊227.5⌋
    }

    #[test]
    fn step_to_follows_the_clock() {
        let mut stepped = ZoneAmbient::new(auto(1.0));
        let mut timed = ZoneAmbient::new(auto(1.0));
        timed.step_to(5_000_000, false, false); // the first call only sets the time
        assert_eq!(timed.quiet_s(), 0.0);
        for i in 1..=150_u64 {
            stepped.step(0.01, false, false);
            timed.step_to(5_000_000 + i * 10_000, false, false);
        }
        assert!((stepped.quiet_s() - timed.quiet_s()).abs() < 1e-9);
        assert!((stepped.amount() - timed.amount()).abs() < 1e-9);
        timed.step_to(1, false, false); // a clock that goes back adds nothing
        assert!((stepped.quiet_s() - timed.quiet_s()).abs() < 1e-9);
    }

    #[test]
    fn settings_come_from_the_zone() {
        let zone: Zone = serde_json::from_value(serde_json::json!({
            "reactive": "auto",
            "ambient": {"color": "#FF3399", "style": "breathe", "level": 0.4,
                        "idle_after": 12, "on_pause": false},
        }))
        .unwrap();
        let settings = AmbientSettings::from_zone(&zone);
        assert_eq!(settings.reactive, Reactive::Auto);
        assert_eq!(settings.color, [0xFF, 0x33, 0x99]);
        assert_eq!(settings.style, AmbientStyle::Breathe);
        assert_eq!(
            (settings.level, settings.idle_after_s, settings.on_pause),
            (0.4, 12.0, false)
        );
        let defaults = AmbientSettings::from_zone(&Zone::default());
        assert_eq!(defaults, AmbientSettings::default());
    }

    #[test]
    fn hex_parsing_is_forgiving() {
        assert_eq!(parse_hex("#0033cc"), [0x00, 0x33, 0xCC]);
        assert_eq!(parse_hex("nonsense"), [0; 3]);
        assert_eq!(parse_hex("#é1234"), [0; 3]);
    }
}
