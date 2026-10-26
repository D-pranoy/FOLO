//! Stage 2: the colour model (ADR-0036 D14, `docs/architecture/capabilities.md`).
//!
//! Colours come in as RGB on the 0–255 scale (linear light already) and leave in the target's
//! colour model, still on that scale.

use folo_protocol::generated::{
    CalibrationWhite, CapabilityDescriptorColorModel, CapabilityLimits, Gamut, OnOff, Xy as GamutXy,
};
use folo_provider::{Cct, ElementColors, RgbCct, Xy};

/// Rec. 709 luminance weights.
pub(crate) const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Colours whose saturation (`(max − min) / max`) is at least this are "saturated" for the CCT
/// rule: they go to an end of the lamp's range instead of the nearest white.
const SATURATED: f32 = 0.5;

/// The CIE xy of D65, which black and the neutral axis take.
const D65: (f32, f32) = (0.3127, 0.3290);

/// Rec. 709 luminance of a colour on the 0–255 scale.
pub(crate) fn luminance(rgb: [f32; 3]) -> f32 {
    LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2]
}

/// How an on/off element switches (`calibration.onoff`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct OnOffRule {
    on_at: f32,
    off_at: f32,
    min_interval_us: u64,
}

impl OnOffRule {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn new(onoff: &OnOff) -> Self {
        // `hysteresis` is the full width of the band around `threshold`.
        let half = (onoff.hysteresis as f32) / 2.0;
        let threshold = onoff.threshold as f32;
        Self {
            on_at: (threshold + half).clamp(0.0, 1.0),
            off_at: (threshold - half).clamp(0.0, 1.0),
            min_interval_us: (onoff.min_interval_ms.max(0.0) * 1000.0) as u64,
        }
    }
}

/// One on/off element's state.
#[derive(Clone, Copy, Debug, Default)]
struct Switch {
    on: bool,
    /// When it last changed; `None` before the first change, which the minimum interval doesn't
    /// hold back.
    changed_us: Option<u64>,
}

/// Everything the colour-model stage keeps and needs.
pub(crate) struct ColorModel {
    kind: CapabilityDescriptorColorModel,
    white: CalibrationWhite,
    cct_range: (f32, f32),
    gamut: [(f32, f32); 3],
    onoff: OnOffRule,
    switches: Vec<Switch>,
}

impl ColorModel {
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn new(
        kind: CapabilityDescriptorColorModel,
        white: CalibrationWhite,
        limits: &CapabilityLimits,
        onoff: &OnOff,
    ) -> Self {
        let (lo, hi) = (limits.cct_min_k as f32, limits.cct_max_k as f32);
        let point = |p: &GamutXy| (p.x as f32, p.y as f32);
        let Gamut { red, green, blue } = &limits.gamut;
        Self {
            kind,
            white,
            cct_range: (lo.min(hi), lo.max(hi)),
            gamut: [point(red), point(green), point(blue)],
            onoff: OnOffRule::new(onoff),
            switches: Vec::new(),
        }
    }

    pub(crate) fn kind(&self) -> CapabilityDescriptorColorModel {
        self.kind
    }

    /// The target's colours for these RGB elements.
    pub(crate) fn convert(&mut self, now_us: u64, rgb: Vec<[f32; 3]>) -> ElementColors {
        use CapabilityDescriptorColorModel as M;
        match self.kind {
            M::Rgb => ElementColors::Rgb(rgb),
            M::Rgbw => ElementColors::Rgbw(
                rgb.into_iter()
                    .map(|c| {
                        let (rgb, w) = extract_white(c, self.white);
                        [rgb[0], rgb[1], rgb[2], w]
                    })
                    .collect(),
            ),
            M::Rgbcct => ElementColors::RgbCct(
                rgb.into_iter()
                    .map(|c| {
                        let (extracted, white) = extract_white(c, self.white);
                        RgbCct {
                            rgb: extracted,
                            white,
                            kelvin: self.kelvin(c),
                        }
                    })
                    .collect(),
            ),
            M::Cct => ElementColors::Cct(
                rgb.into_iter()
                    .map(|c| Cct {
                        brightness: luminance(c),
                        kelvin: self.kelvin(c),
                    })
                    .collect(),
            ),
            M::Dimmer => ElementColors::Dimmer(rgb.into_iter().map(luminance).collect()),
            M::Onoff => self.switch(now_us, &rgb),
            M::Xy => ElementColors::Xy(rgb.into_iter().map(|c| self.xy(c)).collect()),
        }
    }

    /// On/off with hysteresis and a minimum interval, per element, on its level (luminance, 0–1).
    fn switch(&mut self, now_us: u64, rgb: &[[f32; 3]]) -> ElementColors {
        self.switches.resize(rgb.len(), Switch::default());
        let rule = self.onoff;
        let states = rgb
            .iter()
            .zip(&mut self.switches)
            .map(|(c, switch)| {
                let level = luminance(*c) / 255.0;
                // Strict at the on point, so black never switches an element on.
                let want = if switch.on {
                    level >= rule.off_at
                } else {
                    level > rule.on_at
                };
                let ready = switch
                    .changed_us
                    .is_none_or(|t| now_us.saturating_sub(t) >= rule.min_interval_us);
                if want != switch.on && ready {
                    switch.on = want;
                    switch.changed_us = Some(now_us);
                }
                switch.on
            })
            .collect();
        ElementColors::OnOff(states)
    }

    /// The colour temperature of a colour: the nearest white clamped to the lamp's range, and
    /// for saturated colours a fixed rule, the warm end when red is at least blue, else the
    /// cool end.
    fn kelvin(&self, rgb: [f32; 3]) -> f32 {
        let (lo, hi) = self.cct_range;
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        if max <= 0.0 {
            return f32::midpoint(lo, hi);
        }
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        if (max - min) / max >= SATURATED {
            return if rgb[0] >= rgb[2] { lo } else { hi };
        }
        let (x, y) = chromaticity(rgb);
        mccamy(x, y).clamp(lo, hi)
    }

    /// RGB to CIE xy clamped to the gamut, and the brightness component (the luminance).
    fn xy(&self, rgb: [f32; 3]) -> Xy {
        let (x, y) = chromaticity(rgb);
        let (x, y) = clamp_to_gamut((x, y), self.gamut);
        Xy {
            x,
            y,
            brightness: luminance(rgb),
        }
    }
}

/// The white channel's share of a colour, and the colour left for R, G and B.
fn extract_white(rgb: [f32; 3], mode: CalibrationWhite) -> ([f32; 3], f32) {
    let common = rgb[0].min(rgb[1]).min(rgb[2]);
    match mode {
        CalibrationWhite::Off => (rgb, 0.0),
        // The grey part of the colour moves to W, so the colour keeps its look.
        CalibrationWhite::Accurate => (rgb.map(|v| v - common), common),
        // W is added on top: brighter, a little less saturated.
        CalibrationWhite::Brighter => (rgb, common),
    }
}

/// CIE xy of linear sRGB primaries (D65); black gives D65's white point.
#[allow(clippy::many_single_char_names)]
fn chromaticity(rgb: [f32; 3]) -> (f32, f32) {
    let [r, g, b] = rgb;
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let sum = x + y + z;
    if sum <= 0.0 { D65 } else { (x / sum, y / sum) }
}

/// `McCamy`'s approximation of the correlated colour temperature, in kelvin.
fn mccamy(x: f32, y: f32) -> f32 {
    let n = (x - 0.3320) / (0.1858 - y);
    ((449.0 * n + 3525.0) * n + 6823.3) * n + 5520.33
}

fn cross(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

fn inside(p: (f32, f32), t: [(f32, f32); 3]) -> bool {
    let d = [
        cross(t[0], t[1], p),
        cross(t[1], t[2], p),
        cross(t[2], t[0], p),
    ];
    let (neg, pos) = (d.iter().any(|v| *v < 0.0), d.iter().any(|v| *v > 0.0));
    !(neg && pos)
}

fn closest_on_segment(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= 0.0 {
        return a;
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    (a.0 + t * dx, a.1 + t * dy)
}

/// The point itself inside the triangle, else the nearest point of its edges.
fn clamp_to_gamut(p: (f32, f32), t: [(f32, f32); 3]) -> (f32, f32) {
    if inside(p, t) {
        return p;
    }
    let mut best = (f32::INFINITY, p);
    for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
        let q = closest_on_segment(a, b, p);
        let d = (q.0 - p.0).powi(2) + (q.1 - p.1).powi(2);
        if d < best.0 {
            best = (d, q);
        }
    }
    best.1
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

    fn model(kind: CapabilityDescriptorColorModel, white: CalibrationWhite) -> ColorModel {
        ColorModel::new(kind, white, &CapabilityLimits::default(), &OnOff::default())
    }

    fn colors(m: &mut ColorModel, now: u64, c: [f32; 3]) -> ElementColors {
        m.convert(now, vec![c])
    }

    #[test]
    fn rgb_passes_through_bit_for_bit() {
        let mut m = model(
            CapabilityDescriptorColorModel::Rgb,
            CalibrationWhite::Accurate,
        );
        assert_eq!(
            colors(&mut m, 0, [0.3, 254.7, 9.1]),
            ElementColors::Rgb(vec![[0.3, 254.7, 9.1]])
        );
    }

    #[test]
    fn rgbw_extraction_modes() {
        let c = [200.0, 150.0, 100.0];
        let w = |mode| {
            let mut m = model(CapabilityDescriptorColorModel::Rgbw, mode);
            colors(&mut m, 0, c)
        };
        assert_eq!(
            w(CalibrationWhite::Off),
            ElementColors::Rgbw(vec![[200.0, 150.0, 100.0, 0.0]])
        );
        assert_eq!(
            w(CalibrationWhite::Accurate),
            ElementColors::Rgbw(vec![[100.0, 50.0, 0.0, 100.0]])
        );
        assert_eq!(
            w(CalibrationWhite::Brighter),
            ElementColors::Rgbw(vec![[200.0, 150.0, 100.0, 100.0]])
        );
    }

    #[test]
    fn accurate_turns_white_into_w_alone() {
        let mut m = model(
            CapabilityDescriptorColorModel::Rgbw,
            CalibrationWhite::Accurate,
        );
        assert_eq!(
            colors(&mut m, 0, [255.0; 3]),
            ElementColors::Rgbw(vec![[0.0, 0.0, 0.0, 255.0]])
        );
    }

    #[test]
    fn rgbcct_takes_the_white_and_a_temperature() {
        let mut m = model(
            CapabilityDescriptorColorModel::Rgbcct,
            CalibrationWhite::Accurate,
        );
        let ElementColors::RgbCct(v) = colors(&mut m, 0, [255.0, 200.0, 120.0]) else {
            panic!("not RGB+CCT");
        };
        assert_eq!(v[0].rgb, [135.0, 80.0, 0.0]);
        assert_eq!(v[0].white, 120.0);
        assert!(
            (2000.0..4500.0).contains(&v[0].kelvin),
            "warm tint: {}",
            v[0].kelvin
        );
    }

    #[test]
    fn cct_brightness_is_luminance_and_neutral_white_is_cool() {
        let mut m = model(
            CapabilityDescriptorColorModel::Cct,
            CalibrationWhite::Accurate,
        );
        let ElementColors::Cct(v) = colors(&mut m, 0, [255.0; 3]) else {
            panic!("not CCT");
        };
        assert!((v[0].brightness - 255.0).abs() < 0.01);
        // D65 is about 6500 K, the top of the default range.
        assert!(
            v[0].kelvin > 6000.0 && v[0].kelvin <= 6500.0,
            "{}",
            v[0].kelvin
        );
    }

    #[test]
    fn cct_clamps_to_the_lamps_range() {
        let limits = CapabilityLimits {
            cct_min_k: 2700.0,
            cct_max_k: 5000.0,
            ..CapabilityLimits::default()
        };
        let mut m = ColorModel::new(
            CapabilityDescriptorColorModel::Cct,
            CalibrationWhite::Accurate,
            &limits,
            &OnOff::default(),
        );
        let ElementColors::Cct(v) = colors(&mut m, 0, [255.0; 3]) else {
            panic!("not CCT");
        };
        assert_eq!(v[0].kelvin, 5000.0);
    }

    #[test]
    fn saturated_colours_go_to_an_end_of_the_range() {
        let mut m = model(
            CapabilityDescriptorColorModel::Cct,
            CalibrationWhite::Accurate,
        );
        let mut kelvin = |c| match colors(&mut m, 0, c) {
            ElementColors::Cct(v) => v[0].kelvin,
            _ => unreachable!(),
        };
        assert_eq!(kelvin([255.0, 0.0, 0.0]), 2000.0);
        assert_eq!(kelvin([0.0, 0.0, 255.0]), 6500.0);
    }

    #[test]
    fn dimmer_is_luminance() {
        let mut m = model(
            CapabilityDescriptorColorModel::Dimmer,
            CalibrationWhite::Accurate,
        );
        let ElementColors::Dimmer(v) = colors(&mut m, 0, [0.0, 100.0, 0.0]) else {
            panic!("not a dimmer");
        };
        assert!((v[0] - 71.52).abs() < 0.001);
    }

    fn onoff_model(threshold: f64, hysteresis: f64, min_interval_ms: f64) -> ColorModel {
        ColorModel::new(
            CapabilityDescriptorColorModel::Onoff,
            CalibrationWhite::Accurate,
            &CapabilityLimits::default(),
            &OnOff {
                threshold,
                hysteresis,
                min_interval_ms,
            },
        )
    }

    fn on(m: &mut ColorModel, now: u64, level: f32) -> bool {
        match colors(m, now, [level * 255.0; 3]) {
            ElementColors::OnOff(v) => v[0],
            _ => unreachable!(),
        }
    }

    #[test]
    fn onoff_switches_at_the_band_edges_and_keeps_state_between() {
        // Threshold 0.5, hysteresis 0.1: on above 0.55, off below 0.45.
        let mut m = onoff_model(0.5, 0.1, 0.0);
        assert!(!on(&mut m, 0, 0.5), "inside the band, still off");
        assert!(on(&mut m, 1, 0.6));
        assert!(on(&mut m, 2, 0.5), "inside the band, stays on");
        assert!(on(&mut m, 3, 0.46));
        assert!(!on(&mut m, 4, 0.4));
        assert!(!on(&mut m, 5, 0.5), "inside the band, stays off");
    }

    #[test]
    fn onoff_waits_for_the_minimum_interval() {
        let mut m = onoff_model(0.5, 0.1, 1000.0);
        assert!(on(&mut m, 10, 1.0), "the first change isn't held back");
        assert!(on(&mut m, 500_000, 0.0), "too soon to switch off");
        assert!(on(&mut m, 1_000_009, 0.0), "999.999 ms since the change");
        assert!(!on(&mut m, 1_000_010, 0.0), "a second since the change");
        assert!(!on(&mut m, 1_500_000, 1.0), "held again for a second");
        assert!(on(&mut m, 2_010_000, 1.0));
    }

    #[test]
    fn onoff_black_never_switches_on() {
        let mut m = onoff_model(0.0, 0.0, 0.0);
        assert!(!on(&mut m, 0, 0.0));
    }

    #[test]
    fn onoff_thresholds_are_clamped_to_the_unit_range() {
        let rule = OnOffRule::new(&OnOff {
            threshold: 0.98,
            hysteresis: 0.2,
            min_interval_ms: 0.0,
        });
        assert_eq!(rule.on_at, 1.0);
        assert!((rule.off_at - 0.88).abs() < 1e-6);
    }

    #[test]
    fn xy_of_white_is_the_white_point_and_brightness_is_luminance() {
        let mut m = model(
            CapabilityDescriptorColorModel::Xy,
            CalibrationWhite::Accurate,
        );
        let ElementColors::Xy(v) = colors(&mut m, 0, [255.0; 3]) else {
            panic!("not xy");
        };
        assert!((v[0].x - D65.0).abs() < 0.002 && (v[0].y - D65.1).abs() < 0.002);
        assert!((v[0].brightness - 255.0).abs() < 0.01);
        let ElementColors::Xy(black) = colors(&mut m, 0, [0.0; 3]) else {
            panic!("not xy");
        };
        assert_eq!(
            (black[0].x, black[0].y, black[0].brightness),
            (D65.0, D65.1, 0.0)
        );
    }

    #[test]
    fn xy_outside_the_gamut_lands_on_its_edge() {
        // A narrow gamut around white: saturated green is far outside it.
        let limits = CapabilityLimits {
            gamut: Gamut {
                red: GamutXy { x: 0.40, y: 0.35 },
                green: GamutXy { x: 0.30, y: 0.40 },
                blue: GamutXy { x: 0.25, y: 0.25 },
            },
            ..CapabilityLimits::default()
        };
        let mut m = ColorModel::new(
            CapabilityDescriptorColorModel::Xy,
            CalibrationWhite::Accurate,
            &limits,
            &OnOff::default(),
        );
        let ElementColors::Xy(v) = colors(&mut m, 0, [0.0, 255.0, 0.0]) else {
            panic!("not xy");
        };
        let p = (v[0].x, v[0].y);
        // Pure green is (0.30, 0.60), straight above the gamut's green corner.
        assert!(
            (p.0 - 0.30).abs() < 0.01 && (p.1 - 0.40).abs() < 0.01,
            "{p:?}"
        );
        assert!(inside(p, m.gamut));
    }

    #[test]
    fn xy_inside_the_gamut_is_untouched() {
        let m = model(
            CapabilityDescriptorColorModel::Xy,
            CalibrationWhite::Accurate,
        );
        let (x, y) = chromaticity([255.0, 255.0, 255.0]);
        assert_eq!(clamp_to_gamut((x, y), m.gamut), (x, y));
    }
}
