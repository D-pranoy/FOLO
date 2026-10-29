//! Stage 3: calibration (ADR-0036 D15, `docs/architecture/capabilities.md`).
//!
//! Per element, on the 0–255 scale: gamma, white balance, brightness range, colour order, each
//! on the channels its colour model gives it. A calibration of defaults skips the stage.

use folo_protocol::generated::{
    Calibration, CalibrationColorOrder, CapabilityDescriptorColorModel,
};
use folo_provider::ElementColors;

/// The positions of the logical R, G and B in the target's wire order: wire channel `i` carries
/// logical channel `order[i]`.
pub(crate) fn wire_order(order: CalibrationColorOrder) -> [usize; 3] {
    match order {
        CalibrationColorOrder::Rgb => [0, 1, 2],
        CalibrationColorOrder::Rbg => [0, 2, 1],
        CalibrationColorOrder::Grb => [1, 0, 2],
        CalibrationColorOrder::Gbr => [1, 2, 0],
        CalibrationColorOrder::Brg => [2, 0, 1],
        CalibrationColorOrder::Bgr => [2, 1, 0],
    }
}

/// A calibration worked out for one target.
#[derive(Clone, Debug)]
pub(crate) struct Calibrator {
    /// `None` for gamma 1.
    gamma: Option<f32>,
    /// `None` for factors of 1.
    balance: Option<[f32; 3]>,
    /// The `[min, max]` brightness range as fractions of the full scale; `None` for `[0, 1]`.
    range: Option<(f32, f32)>,
    /// `None` for `rgb`.
    order: Option<[usize; 3]>,
}

impl Calibrator {
    /// `min_brightness` is the target's `limits.min_brightness`: the range's minimum is the
    /// larger of that and `brightness_min`.
    // Defaults are compared exactly on purpose: a calibration of defaults must skip the stage.
    #[allow(clippy::cast_possible_truncation, clippy::float_cmp)]
    pub(crate) fn new(calibration: &Calibration, min_brightness: f64) -> Self {
        let gamma = calibration.gamma as f32;
        let balance = [
            calibration.white_balance.r as f32,
            calibration.white_balance.g as f32,
            calibration.white_balance.b as f32,
        ];
        let min = (calibration.brightness_min.max(min_brightness) as f32).clamp(0.0, 1.0);
        let max = (calibration.brightness_max as f32).clamp(min, 1.0);
        let order = wire_order(calibration.color_order);
        Self {
            gamma: (gamma > 0.0 && gamma != 1.0).then_some(gamma),
            balance: (balance != [1.0; 3]).then_some(balance),
            range: (min != 0.0 || max != 1.0).then_some((min, max)),
            order: (order != [0, 1, 2]).then_some(order),
        }
    }

    /// Whether the stage changes nothing.
    pub(crate) fn is_default(&self) -> bool {
        self.gamma.is_none()
            && self.balance.is_none()
            && self.range.is_none()
            && self.order.is_none()
    }

    /// Gamma, then white balance (`factor`), then the brightness range, on one channel.
    fn channel(&self, value: f32, factor: Option<f32>) -> f32 {
        let mut v = value.clamp(0.0, 255.0);
        if let Some(g) = self.gamma {
            v = 255.0 * (v / 255.0).powf(g);
        }
        if let Some(f) = factor {
            v = (v * f).clamp(0.0, 255.0);
        }
        self.brightness(v)
    }

    /// The brightness range alone: non-zero values map into `[min, max]`, zero stays zero.
    fn brightness(&self, v: f32) -> f32 {
        match self.range {
            Some((min, max)) if v > 0.0 => 255.0 * min + (max - min) * v,
            _ => v,
        }
    }

    /// Gamma and the brightness range on a channel white balance and order don't touch: a white,
    /// a CCT level, a dimmer's or xy's brightness.
    fn level(&self, v: f32) -> f32 {
        self.channel(v, None)
    }

    fn rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        let shaped = std::array::from_fn(|i| self.channel(rgb[i], self.balance.map(|b| b[i])));
        match self.order {
            Some(order) => order.map(|i| shaped[i]),
            None => shaped,
        }
    }

    /// Calibrates the colours of a target in place.
    pub(crate) fn apply(&self, colors: &mut ElementColors) {
        if self.is_default() {
            return;
        }
        match colors {
            ElementColors::Rgb(v) => v.iter_mut().for_each(|c| *c = self.rgb(*c)),
            ElementColors::Rgbw(v) => v.iter_mut().for_each(|c| {
                let [r, g, b] = self.rgb([c[0], c[1], c[2]]);
                *c = [r, g, b, self.level(c[3])];
            }),
            ElementColors::RgbCct(v) => v.iter_mut().for_each(|c| {
                c.rgb = self.rgb(c.rgb);
                c.white = self.level(c.white);
            }),
            ElementColors::Cct(v) => v
                .iter_mut()
                .for_each(|c| c.brightness = self.level(c.brightness)),
            ElementColors::Dimmer(v) => v.iter_mut().for_each(|c| *c = self.level(*c)),
            ElementColors::Xy(v) => v
                .iter_mut()
                .for_each(|c| c.brightness = self.level(c.brightness)),
            // `calibration.onoff` governs these.
            ElementColors::OnOff(_) => {}
        }
    }
}

/// Whether a colour model's channels take the colour order (and so the limiter undoes it).
pub(crate) fn has_order(kind: CapabilityDescriptorColorModel) -> bool {
    use CapabilityDescriptorColorModel as M;
    matches!(kind, M::Rgb | M::Rgbw | M::Rgbcct)
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
    use folo_protocol::generated::WhiteBalance;
    use folo_provider::{Cct, RgbCct, Xy};

    fn cal(f: impl FnOnce(&mut Calibration)) -> Calibrator {
        let mut c = Calibration::default();
        f(&mut c);
        Calibrator::new(&c, 0.0)
    }

    fn rgb(c: &Calibrator, v: [f32; 3]) -> [f32; 3] {
        let mut colors = ElementColors::Rgb(vec![v]);
        c.apply(&mut colors);
        let ElementColors::Rgb(out) = colors else {
            unreachable!()
        };
        out[0]
    }

    #[test]
    fn defaults_skip_the_stage_and_change_nothing() {
        let c = cal(|_| {});
        assert!(c.is_default());
        assert_eq!(rgb(&c, [0.3, 127.77, 255.0]), [0.3, 127.77, 255.0]);
    }

    #[test]
    fn gamma_follows_the_formula() {
        let c = cal(|c| c.gamma = 2.0);
        let out = rgb(&c, [0.0, 127.5, 255.0]);
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 63.75).abs() < 0.01, "{}", out[1]);
        assert!((out[2] - 255.0).abs() < 0.01);
    }

    #[test]
    fn white_balance_scales_each_channel() {
        let c = cal(|c| {
            c.white_balance = WhiteBalance {
                r: 1.0,
                g: 0.5,
                b: 0.25,
            };
        });
        assert_eq!(rgb(&c, [100.0, 100.0, 100.0]), [100.0, 50.0, 25.0]);
        assert_eq!(rgb(&c, [255.0; 3])[0], 255.0);
    }

    #[test]
    fn brightness_range_maps_non_zero_values_and_keeps_zero() {
        let c = cal(|c| {
            c.brightness_min = 0.1;
            c.brightness_max = 0.5;
        });
        let out = rgb(&c, [0.0, 255.0, 127.5]);
        assert_eq!(out[0], 0.0);
        assert!(
            (out[1] - 127.5).abs() < 0.01,
            "full scale is the maximum: {}",
            out[1]
        );
        // 0.1·255 + 0.4·127.5
        assert!((out[2] - 76.5).abs() < 0.01, "{}", out[2]);
        let tiny = rgb(&c, [0.5, 0.0, 0.0])[0];
        assert!(
            tiny > 25.0,
            "the lowest non-zero value sits at the minimum: {tiny}"
        );
    }

    #[test]
    fn the_minimum_is_the_larger_of_the_calibration_and_the_lamps_limit() {
        let calibration = Calibration {
            brightness_min: 0.05,
            ..Calibration::default()
        };
        let c = Calibrator::new(&calibration, 0.2);
        let out = rgb(&c, [1.0, 0.0, 0.0])[0];
        assert!(out > 51.0 && out < 52.5, "{out}");
        assert!(!Calibrator::new(&Calibration::default(), 0.2).is_default());
    }

    #[test]
    fn colour_order_permutes_the_channels() {
        let c = cal(|c| c.color_order = CalibrationColorOrder::Grb);
        assert_eq!(rgb(&c, [1.0, 2.0, 3.0]), [2.0, 1.0, 3.0]);
        let c = cal(|c| c.color_order = CalibrationColorOrder::Brg);
        assert_eq!(rgb(&c, [1.0, 2.0, 3.0]), [3.0, 1.0, 2.0]);
        let c = cal(|c| c.color_order = CalibrationColorOrder::Bgr);
        assert_eq!(rgb(&c, [1.0, 2.0, 3.0]), [3.0, 2.0, 1.0]);
    }

    #[test]
    fn an_order_alone_keeps_values_exact() {
        let c = cal(|c| c.color_order = CalibrationColorOrder::Gbr);
        assert_eq!(rgb(&c, [0.3, 127.77, 254.9]), [127.77, 254.9, 0.3]);
    }

    #[test]
    fn steps_apply_in_order_gamma_balance_range() {
        let c = cal(|c| {
            c.gamma = 2.0;
            c.white_balance = WhiteBalance {
                r: 0.5,
                g: 1.0,
                b: 1.0,
            };
            c.brightness_max = 0.5;
        });
        // 255 → gamma 255 → balance 127.5 → range ×0.5 = 63.75
        assert!((rgb(&c, [255.0, 0.0, 0.0])[0] - 63.75).abs() < 0.01);
    }

    #[test]
    fn rgbw_gamma_touches_white_but_balance_and_order_do_not() {
        let c = cal(|c| {
            c.gamma = 2.0;
            c.white_balance = WhiteBalance {
                r: 0.5,
                g: 0.5,
                b: 0.5,
            };
            c.color_order = CalibrationColorOrder::Grb;
        });
        let mut colors = ElementColors::Rgbw(vec![[255.0, 0.0, 0.0, 127.5]]);
        c.apply(&mut colors);
        let ElementColors::Rgbw(v) = colors else {
            unreachable!()
        };
        assert!((v[0][0]).abs() < 1e-3, "red became the second wire channel");
        assert!((v[0][1] - 127.5).abs() < 0.01);
        assert!((v[0][3] - 63.75).abs() < 0.01, "W: gamma only");
    }

    #[test]
    fn rgbcct_leaves_the_temperature_alone() {
        let c = cal(|c| c.gamma = 2.0);
        let mut colors = ElementColors::RgbCct(vec![RgbCct {
            rgb: [0.0; 3],
            white: 127.5,
            kelvin: 3000.0,
        }]);
        c.apply(&mut colors);
        let ElementColors::RgbCct(v) = colors else {
            unreachable!()
        };
        assert!((v[0].white - 63.75).abs() < 0.01);
        assert_eq!(v[0].kelvin, 3000.0);
    }

    #[test]
    fn cct_dimmer_and_xy_take_gamma_on_the_brightness_only() {
        let c = cal(|c| {
            c.gamma = 2.0;
            c.color_order = CalibrationColorOrder::Bgr;
        });
        let mut cct = ElementColors::Cct(vec![Cct {
            brightness: 127.5,
            kelvin: 4000.0,
        }]);
        c.apply(&mut cct);
        let ElementColors::Cct(v) = cct else {
            unreachable!()
        };
        assert!((v[0].brightness - 63.75).abs() < 0.01);
        assert_eq!(v[0].kelvin, 4000.0);

        let mut dim = ElementColors::Dimmer(vec![127.5]);
        c.apply(&mut dim);
        assert_eq!(dim, ElementColors::Dimmer(vec![63.75]));

        let mut xy = ElementColors::Xy(vec![Xy {
            x: 0.3,
            y: 0.3,
            brightness: 127.5,
        }]);
        c.apply(&mut xy);
        let ElementColors::Xy(v) = xy else {
            unreachable!()
        };
        assert_eq!((v[0].x, v[0].y), (0.3, 0.3));
        assert!((v[0].brightness - 63.75).abs() < 0.01);
    }

    #[test]
    fn onoff_is_untouched() {
        let c = cal(|c| {
            c.gamma = 2.0;
            c.brightness_min = 0.5;
        });
        let mut colors = ElementColors::OnOff(vec![true, false]);
        c.apply(&mut colors);
        assert_eq!(colors, ElementColors::OnOff(vec![true, false]));
    }
}
