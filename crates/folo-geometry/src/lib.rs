//! Zone geometry and the CPU projector (RB-08 steps 1 and 2).
//!
//! - [`element_layout`] says where each element of a target is in the zone, for points, lines and
//!   grids (`display` targets are validated, never executed).
//! - [`validate`] checks a zone's targets, surfaces and mappings.
//! - A [`GatherPlan`] is what one mapping reads of a surface for each element, made by the
//!   nearest, bilinear or area [kernels](kernel::MAX_TAPS) and kept per configuration revision.
//! - The [`Projector`] trait turns surface frames into a target's element colours; [`CpuProjector`]
//!   gathers on the CPU.
//!
//! The decisions are ADR-0036 rows 10 to 12 and [`docs/architecture/spatial-model.md`].
//!
//! [`docs/architecture/spatial-model.md`]: ../../../docs/architecture/spatial-model.md

// A clamped zone has bounded sizes (a grid up to 256 × 256 elements, a strip up to 2000, a canvas
// published at up to 128 × 72 cells), so casts between usize, u32 and f64 are exact, and a float
// coordinate is floored on purpose. `element_layout` and the projector expect a clamped zone.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod build;
mod kernel;
mod layout;
mod math;
mod plan;
mod project;
mod validate;

pub use kernel::MAX_TAPS;
pub use layout::{Layout, Shape, element_layout, wiring_cell};
pub use math::Vec3;
pub use plan::{CANVAS_PUBLISHED_MAX, GatherPlan, published_size};
pub use project::{CpuProjector, MappingPlan, Prepared, ProjectError, Projector, SurfaceView};
pub use validate::{Issue, Severity, validate};
