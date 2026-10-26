//! The adapter chain and the flash limiter (RB-08 step 4, ADR-0036 D1 and D14–D18).
//!
//! One [`AdapterChain`] per target turns the projected elements (RGB on the effect engine's
//! 0–255 scale) into the colours the target's provider takes, in this order
//! (`docs/architecture/capabilities.md#adapter-chain-per-target-in-order`):
//!
//! 1. **resolution:** fit to `address.count`
//! 2. **colour model:** RGB, RGBW and RGB+CCT with white extraction, CCT, dimmer, on/off with
//!    hysteresis and a minimum interval, xy within a gamut
//! 3. **calibration:** gamma, white balance, brightness range and colour order
//! 4. **latency:** the delay line
//! 5. **flash limiter:** per element, for zones that switched it on
//!
//! The rate isn't a stage: the provider's sender applies it afterwards
//! ([`folo_provider::spawn_sender`]). Nothing here keeps time of its own; every call takes the
//! controller's clock, so tests drive it with theirs.

mod calibration;
mod delay;
mod limiter;
mod model;
mod resolution;

use folo_protocol::generated::{CapabilityDescriptorColorModel, Target};
use folo_provider::{ElementColors, effective_capabilities};

use calibration::{Calibrator, has_order, wire_order};
use delay::DelayLine;
use limiter::FlashLimiter;
use model::ColorModel;

pub use delay::MAX_DELAY_MS;

/// A target's delay in milliseconds: its `calibration.latency_ms` plus what it waits for the
/// slowest target of its zone (`slowest_ms`, the largest `limits.latency_ms` among the zone's
/// targets) minus its own `limits.latency_ms`, between 0 and 5 s (ADR-0036 D17).
#[must_use]
pub fn delay_ms(target: &Target, slowest_ms: f64) -> f64 {
    let own = effective_capabilities(&target.capabilities)
        .limits
        .latency_ms;
    let wait = (slowest_ms - own).max(0.0);
    (target.calibration.latency_ms + wait).clamp(0.0, MAX_DELAY_MS)
}

/// The stages one target's colours go through.
pub struct AdapterChain {
    count: usize,
    model: ColorModel,
    calibrator: Calibrator,
    delay: DelayLine,
    limiter: Option<FlashLimiter>,
}

impl AdapterChain {
    /// The chain of `target`. `flash_limiter` is the zone's setting; `delay_ms` is the target's
    /// total delay, as [`delay_ms`] works it out (0 to 5 s).
    #[must_use]
    pub fn new(target: &Target, flash_limiter: bool, delay_ms: f64) -> Self {
        let capabilities = effective_capabilities(&target.capabilities);
        let calibration = &target.calibration;
        let model = ColorModel::new(
            capabilities.color_model,
            calibration.white,
            &capabilities.limits,
            &calibration.onoff,
        );
        let calibrator = Calibrator::new(calibration, capabilities.limits.min_brightness);
        // The schema's maximum is 65535, which any usize holds.
        let count = usize::try_from(target.provider.address.count.get()).unwrap_or(1);
        let order = if has_order(model.kind()) {
            wire_order(calibration.color_order)
        } else {
            [0, 1, 2]
        };
        Self {
            count,
            model,
            calibrator,
            delay: DelayLine::new(delay_ms),
            limiter: flash_limiter.then(|| FlashLimiter::new(order)),
        }
    }

    /// Whether the chain changes nothing it is given, apart from fitting the elements to
    /// `address.count`, which depends on what `run` gets: an RGB target with a calibration of
    /// defaults, no delay and no limiter. Parity mode relies on this ([pipeline][p]).
    ///
    /// [p]: https://github.com/D-pranoy/FOLO/blob/v1/docs/architecture/pipeline.md#parity-mode
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.model.kind() == CapabilityDescriptorColorModel::Rgb
            && self.calibrator.is_default()
            && self.delay.is_none()
            && self.limiter.is_none()
    }

    /// The target's colours for the projected elements of the step at `now_us`.
    pub fn run(&mut self, now_us: u64, input: &[[f32; 3]]) -> ElementColors {
        let resolved = resolution::fit(input, self.count);
        let mut colors = self.model.convert(now_us, resolved);
        self.calibrator.apply(&mut colors);
        let mut colors = self.delay.run(now_us, colors);
        if let Some(limiter) = &mut self.limiter {
            limiter.apply(now_us, &mut colors);
        }
        colors
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
    use folo_protocol::generated::{
        CalibrationColorOrder, CapabilityOverrides, CapabilityOverridesColorModel,
    };

    fn target(count: u64) -> Target {
        let mut t = Target::default();
        t.provider.address.count = count.try_into().unwrap();
        t
    }

    fn with_model(mut t: Target, model: CapabilityOverridesColorModel) -> Target {
        t.capabilities.overrides = CapabilityOverrides {
            color_model: Some(model),
            ..CapabilityOverrides::default()
        };
        t
    }

    const STEP: u64 = 16_667;

    #[test]
    fn an_identity_chain_reproduces_its_input_exactly() {
        let mut chain = AdapterChain::new(&target(5), false, 0.0);
        assert!(chain.is_identity());
        let input: Vec<[f32; 3]> = (0..5)
            .map(|i| [i as f32 * 63.7 + 0.31, 255.0 - i as f32 * 0.013, 127.499_99])
            .collect();
        for step in 0..3 {
            let out = chain.run(step * STEP, &input);
            assert_eq!(out, ElementColors::Rgb(input.clone()));
            assert_eq!(
                out.to_bytes(),
                input.iter().flatten().map(|v| *v as u8).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn what_breaks_identity() {
        let t = target(2);
        assert!(!AdapterChain::new(&t, true, 0.0).is_identity());
        assert!(!AdapterChain::new(&t, false, 10.0).is_identity());
        assert!(
            !AdapterChain::new(
                &with_model(t.clone(), CapabilityOverridesColorModel::Rgbw),
                false,
                0.0
            )
            .is_identity()
        );
        let mut t2 = t.clone();
        t2.calibration.gamma = 2.2;
        assert!(!AdapterChain::new(&t2, false, 0.0).is_identity());
        let mut t3 = t;
        t3.calibration.color_order = CalibrationColorOrder::Grb;
        assert!(!AdapterChain::new(&t3, false, 0.0).is_identity());
    }

    #[test]
    fn the_chain_fits_resolves_converts_and_calibrates_in_order() {
        // Four projected elements onto two RGBW addresses, with gamma 2: the mean comes first,
        // then white extraction, then gamma on R, G, B and W.
        let mut t = with_model(target(2), CapabilityOverridesColorModel::Rgbw);
        t.calibration.gamma = 2.0;
        let mut chain = AdapterChain::new(&t, false, 0.0);
        let input = [
            [255.0, 255.0, 255.0],
            [255.0, 255.0, 255.0],
            [0.0; 3],
            [0.0; 3],
        ];
        let ElementColors::Rgbw(v) = chain.run(0, &input) else {
            panic!("not RGBW")
        };
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], [0.0, 0.0, 0.0, 255.0]);
        assert_eq!(v[1], [0.0; 4]);
        // A grey of 127.5 is W 127.5 after extraction, and 63.75 after gamma 2.
        let ElementColors::Rgbw(v) = chain.run(STEP, &[[127.5; 3]; 4]) else {
            panic!("not RGBW")
        };
        assert!((v[0][3] - 63.75).abs() < 0.01 && v[0][0] == 0.0);
    }

    #[test]
    fn the_delay_comes_after_calibration_and_before_the_limiter() {
        let mut t = target(1);
        t.calibration.color_order = CalibrationColorOrder::Bgr;
        let mut chain = AdapterChain::new(&t, false, 100.0);
        assert_eq!(
            chain.run(0, &[[1.0, 2.0, 3.0]]),
            ElementColors::Rgb(vec![[3.0, 2.0, 1.0]])
        );
        assert_eq!(
            chain.run(50_000, &[[9.0, 9.0, 9.0]]),
            ElementColors::Rgb(vec![[3.0, 2.0, 1.0]])
        );
        assert_eq!(
            chain.run(100_000, &[[9.0, 9.0, 9.0]]),
            ElementColors::Rgb(vec![[3.0, 2.0, 1.0]])
        );
        assert_eq!(
            chain.run(160_000, &[[9.0, 9.0, 9.0]]),
            ElementColors::Rgb(vec![[9.0, 9.0, 9.0]])
        );
    }

    #[test]
    fn a_one_hertz_pattern_goes_through_the_full_chain_untouched() {
        // An RGBW strip with calibration, a delay and the limiter on; white blinking at 1 Hz.
        let mut t = with_model(target(3), CapabilityOverridesColorModel::Rgbw);
        t.calibration.gamma = 1.8;
        t.calibration.color_order = CalibrationColorOrder::Grb;
        let mut limited = AdapterChain::new(&t, true, 80.0);
        let mut plain = AdapterChain::new(&t, false, 80.0);
        for i in 0..(5 * 60) {
            let now = i * STEP;
            let on = (now / 500_000) % 2 == 1;
            let v = if on { 255.0 } else { 0.0 };
            let input = [[v; 3]; 3];
            assert_eq!(limited.run(now, &input), plain.run(now, &input), "step {i}");
        }
    }

    #[test]
    fn fast_flashing_through_the_full_chain_is_limited_and_slow_content_is_not() {
        let t = target(1);
        let mut limited = AdapterChain::new(&t, true, 0.0);
        let mut changed = false;
        for i in 0..(3 * 60) {
            let v = if (i / 3) % 2 == 1 { 255.0 } else { 0.0 }; // 10 Hz
            let out = limited.run(i * STEP, &[[v; 3]]);
            changed |= out != ElementColors::Rgb(vec![[v; 3]]);
        }
        assert!(changed);
    }

    #[test]
    fn on_off_targets_switch_through_the_chain() {
        let mut t = with_model(target(1), CapabilityOverridesColorModel::Onoff);
        t.calibration.onoff.min_interval_ms = 0.0;
        let mut chain = AdapterChain::new(&t, false, 0.0);
        assert_eq!(
            chain.run(0, &[[255.0; 3]]),
            ElementColors::OnOff(vec![true])
        );
        assert_eq!(chain.run(1, &[[0.0; 3]]), ElementColors::OnOff(vec![false]));
    }

    #[test]
    fn the_delay_adds_the_wait_for_the_slowest_target() {
        let mut t = Target::default();
        t.calibration.latency_ms = 20.0;
        // The default descriptor's latency is its own; the zone's slowest is 120 ms more.
        let own = effective_capabilities(&t.capabilities).limits.latency_ms;
        assert_eq!(delay_ms(&t, own + 120.0), 140.0);
        assert_eq!(delay_ms(&t, 0.0), 20.0, "never negative");
        t.calibration.latency_ms = 4000.0;
        assert_eq!(delay_ms(&t, own + 3000.0), 5000.0, "limited to 5 s");
    }

    /// Blinks `high` / `low` (full-scale logical RGB) at 10 Hz through a limited chain and says
    /// whether anything was held.
    fn held_at_ten_hertz(t: &Target, high: [f32; 3]) -> bool {
        let mut chain = AdapterChain::new(t, true, 0.0);
        let mut plain = AdapterChain::new(t, false, 0.0);
        (0..360u64).any(|i| {
            let v = if (i / 3) % 2 == 1 { high } else { [0.0; 3] };
            chain.run(i * STEP, &[v]) != plain.run(i * STEP, &[v])
        })
    }

    #[test]
    fn the_limiter_undoes_the_colour_order_set_by_the_chain() {
        // Logical blue swings 0.0722: under the threshold. On the wire (BGR) it is channel 0,
        // which an unaware limiter would read as red (0.2126) and hold.
        let mut t = target(1);
        t.calibration.color_order = CalibrationColorOrder::Bgr;
        assert!(!held_at_ten_hertz(&t, [0.0, 0.0, 255.0]));
        // Logical red swings 0.2126 and is held.
        assert!(held_at_ten_hertz(&t, [255.0, 0.0, 0.0]));
    }

    #[test]
    fn the_limiter_judges_what_comes_out_of_calibration() {
        // A brightness range of 0 to 0.05 makes a full-range blink a swing of 0.05.
        let mut t = target(1);
        t.calibration.brightness_max = 0.05;
        assert!(!held_at_ten_hertz(&t, [255.0; 3]));
        assert!(held_at_ten_hertz(&target(1), [255.0; 3]));
    }
}
