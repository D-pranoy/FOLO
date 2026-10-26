//! The provider interface (RB-08 step 6, ADR-0036, `docs/architecture/providers.md`).
//!
//! A *provider* drives one kind of device. This crate holds what every provider
//! implements and what the controller does with it:
//!
//! - the data a provider receives and describes: [`ElementFrame`], [`ElementColors`],
//!   [`DescribedTarget`], [`DeliveryPolicy`]
//! - the traits: [`ProviderKind`] (discovery and making instances), [`Provider`] (an instance:
//!   connect, describe, administration), [`ElementOutput`] (start, send, wire key, release) and
//!   the reserved [`RasterOutput`]
//! - the [`Registry`] of the kinds a node has, and [`effective_capabilities`]
//! - [`spawn_sender`]: one task per instance that keeps it connected, starts and releases its
//!   targets, and sends only changed frames, plus a keepalive, or token-bucketed updates
//! - [`testing::RecordingProvider`], a double for the tests of the provider crates and the node
//!
//! Nothing here talks to a device or to the node; the node wires it in.

mod capabilities;
mod error;
mod registry;
mod sender;
mod traits;
mod types;

pub mod testing;

pub use capabilities::effective_capabilities;
pub use error::{AdminError, ProviderError};
pub use registry::{CreateError, Registry, not_implemented_message, runbook};
pub use sender::{Backoff, SenderConfig, SenderHandle, SenderTarget, spawn_sender};
pub use traits::{AdminMethod, ElementOutput, Provider, ProviderKind, RasterOutput};
pub use types::{
    BoxFuture, BucketSpec, Cct, Delivery, DeliveryPolicy, DescribedLayout, DescribedTarget,
    ElementColors, ElementFrame, ProviderState, ProviderStatus, RgbCct, TargetOutput, Xy,
};
