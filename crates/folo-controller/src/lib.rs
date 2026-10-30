//! The controller's output core (RB-08, [ADR-0036], [pipeline: controller stages]).
//!
//! [`OutputCore::step`] runs the controller's stages for every effectively active zone:
//!
//! 1. the [compositor](folo_compositor::Compositor) picks what each surface shows from its
//!    sources' frames
//! 2. the zone's [ambient light](folo_compositor::ZoneAmbient) is blended into each composited
//!    surface, before projection (ADR-0036 D7)
//! 3. the [projector](folo_geometry::CpuProjector) gathers each target's elements with the plans
//!    of the configuration's revision
//! 4. each target's [adapter chain](folo_adapters::AdapterChain) turns them into its colour
//!    model, with calibration, the delay line and the flash limiter
//!
//! and hands the result per provider instance as [`ElementFrame`](folo_provider::ElementFrame)s,
//! for the node to submit to the instances' senders. A running [test pattern](Pattern) takes the
//! place of a target's projected colours and goes through the same chain.
//!
//! Nothing here reads a clock, starts a task or talks to a device: the node feeds frames,
//! activity and now playing in, steps the core when a frame arrives and at least every 1/60 s,
//! and tests step it with a clock of their own.
//!
//! [ADR-0036]: ../../../docs/decisions/ADR-0036-controller-output.md
//! [pipeline: controller stages]: ../../../docs/architecture/pipeline.md#controller-stages

mod core;
mod pattern;

pub use core::{Media, OutputCore, PatternError, StepOutput, ZoneLive};
pub use pattern::Pattern;
