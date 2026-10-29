//! The display capture agent's core (RB-14, ADR-0037 D20): everything of
//! `folo-capture` that needs no operating system. Safe code only (D19): the
//! backends that call into Windows or the portal live in their own crates.
//!
//! - [`types`]: screens, capture requests, the agent's status, flags and bars
//! - [`backend`] and [`error`]: the [`backend::CaptureBackend`] trait and its errors
//! - [`codec`]: the `FOLC` screen frame of docs/specs/source-link.md#screen-frames
//! - [`color`] and [`sizing`]: the reference for linear-light downscaling and
//!   the frame size that fits a box (D5, D6)
//! - [`pacer`]: sequence numbers, the rate limit, repeats and `held` (D5, D13)
//! - [`synthetic`] and [`replay`]: the backends the tests use

pub mod backend;
pub mod clock;
pub mod codec;
pub mod color;
pub mod error;
pub mod pacer;
pub mod replay;
pub mod sizing;
pub mod synthetic;
pub mod types;
