//! The controller's compositor (RB-08 step 3, [pipeline](../../../docs/architecture/pipeline.md#controller-stages)):
//! what a surface shows from its sources, and the zone's ambient light.
//!
//! - [`Compositor`] keeps the newest frame of every source of every surface and
//!   picks, per surface, what to show: the highest-priority active source, a
//!   300 ms crossfade when that changes, optional blending of all active
//!   sources, and a 500 ms staleness timeout.
//! - [`ZoneAmbient`] is a zone's reactive mode and ambient light: on, auto or
//!   off, a solid or breathing colour, the 1.5 s eased fade in and the 0.15 s
//!   fade out, and the silence it counts from the sources' activity.
//!
//! Nothing here reads a clock or starts a thread: every call is given the time
//! (`now_us`, any monotonic microsecond clock, or a step in seconds), so tests
//! drive the core step by step. Pixels are `f32` on the engine's 0–255 scale.

mod ambient;
mod compositor;

pub use ambient::{
    AmbientSettings, AmbientStyle, BREATHE_LOW, BREATHE_PERIOD_S, FADE_IN_S, FADE_OUT_S,
    PAUSE_QUIET_S, Reactive, ZoneAmbient,
};
pub use compositor::{
    ACTIVITY_HOLD_US, CROSSFADE_US, Composed, Compositor, FrameOutcome, FrameRefused, STALE_US,
    SurfaceSettings,
};
