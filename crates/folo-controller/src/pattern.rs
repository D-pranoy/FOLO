//! The test patterns (`docs/specs/client-api.md#test-patterns`, ADR-0036 D20): what a target
//! shows while a pattern runs on it, in place of its projected colours.

use folo_geometry::Layout;
use folo_protocol::generated::Bounds;

/// White at 25%, on the 0–255 scale.
const QUARTER: f32 = 255.0 * 0.25;
/// `identify` pulses once a second.
const IDENTIFY_PERIOD_US: u64 = 1_000_000;
/// `sweep` passes over the elements once in 2 s.
const SWEEP_PERIOD_US: u64 = 2_000_000;

/// A test pattern (`TestRequest.pattern`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// White at 25%, pulsing between 0 and 25% at 1 Hz.
    Identify,
    /// One element lit white at 25%, moving along the elements in their order, one pass in 2 s.
    Sweep,
    /// Each element's position in the zone's box as red, green and blue.
    Position,
}

impl Pattern {
    /// The pattern a `TestRequest` names, or `None` for another name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "identify" => Some(Self::Identify),
            "sweep" => Some(Self::Sweep),
            "position" => Some(Self::Position),
            _ => None,
        }
    }

    /// The colours of a target's elements `elapsed_us` after the pattern started.
    // The phases are below 2^53 µs and the element index below 2^16: the casts are exact
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub(crate) fn render(
        self,
        elapsed_us: u64,
        layout: &Layout,
        bounds: &Bounds,
        out: &mut [[f32; 3]],
    ) {
        match self {
            Self::Identify => {
                // a raised cosine: dark at the start of each second, 25% halfway
                let phase = (elapsed_us % IDENTIFY_PERIOD_US) as f64 / IDENTIFY_PERIOD_US as f64;
                let level = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * phase).cos();
                out.fill([QUARTER * level as f32; 3]);
            }
            Self::Sweep => {
                out.fill([0.0; 3]);
                let phase = (elapsed_us % SWEEP_PERIOD_US) as f64 / SWEEP_PERIOD_US as f64;
                let lit = (phase * out.len() as f64) as usize;
                if let Some(element) = out.get_mut(lit) {
                    *element = [QUARTER; 3];
                }
            }
            Self::Position => {
                let unit = |v: f64, size: f64| {
                    if size > 0.0 {
                        (v / size).clamp(0.0, 1.0) as f32 * 255.0
                    } else {
                        0.0
                    }
                };
                for (i, element) in out.iter_mut().enumerate() {
                    *element = layout.position(i).map_or([0.0; 3], |p| {
                        [
                            unit(p[0] + bounds.x / 2.0, bounds.x),
                            unit(p[1] + bounds.y / 2.0, bounds.y),
                            unit(p[2], bounds.z),
                        ]
                    });
                }
            }
        }
    }
}
