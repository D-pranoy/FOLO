//! The Linux capture backend of the display capture agent (RB-14,
//! ADR-0037): the xdg-desktop-portal `ScreenCast` interface for consent and
//! restore tokens, and a PipeWire stream with shared-memory buffers,
//! downscaled on the CPU. The crate is empty on other platforms.
//!
//! - [`clock::MonotonicClock`]: `CLOCK_MONOTONIC` in microseconds (D6)
//! - [`scale`]: the CPU downscale (D17)
//! - [`appid`]: the application id the portal knows the agent by (D16)
//! - with the `portal` feature, [`backend::PortalBackend`], the
//!   `CaptureBackend`; the feature needs PipeWire's development files
//!
//! Safe code only: the PipeWire and D-Bus bindings are other crates' FFI, and
//! the clock goes through `rustix` (D19).
#![cfg(target_os = "linux")]

pub mod appid;
pub mod clock;
pub mod scale;

#[cfg(feature = "portal")]
pub mod backend;
#[cfg(feature = "portal")]
mod handoff;
#[cfg(feature = "portal")]
mod notify;
#[cfg(feature = "portal")]
pub mod session;
#[cfg(feature = "portal")]
mod shared;
#[cfg(feature = "portal")]
mod sink;
#[cfg(feature = "portal")]
mod stream;

pub use clock::MonotonicClock;

#[cfg(feature = "portal")]
pub use backend::{BACKEND_NAME, PortalBackend, SCREEN_ID};
#[cfg(feature = "portal")]
pub use session::PortalSettings;
