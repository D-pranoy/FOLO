//! The Hyperion.NG provider (RB-10, [ADR-0011](../../docs/decisions/ADR-0011-hyperion-provider.md),
//! `docs/integrations/hyperion.md`), on the provider interface of `folo-provider`.
//!
//! - [`HyperionKind`] makes a [`HyperionProvider`] per instance of `providers.json`.
//! - The provider speaks Hyperion's JSON API over TCP: `color` frames with the instance's
//!   priority and origin and `duration` 2000 ms, written as the legacy sender writes them
//!   ([`wire`]), a reader that drains every reply, and `clear` when a target is released.
//! - `describe` makes one line target per running Hyperion instance, with the LED rectangles of
//!   `serverinfo` as a layout on the default TV plane ([`layout`]).
//! - [`layout`] also holds the proportional scaling for a changed LED count and the check that
//!   finds the change between two describes.
//! - Administration is in [`admin`].
//! - With the `testing` feature, [`testing::FakeHyperion`] is a Hyperion for tests.
//!
//! The sender, state mapping and wiring into the node are in `folo-provider` and the node.

pub mod admin;
mod kind;
pub mod layout;
mod link;
mod provider;
pub mod serverinfo;
mod settings;
pub mod wire;

#[cfg(feature = "testing")]
pub mod testing;

pub use kind::HyperionKind;
pub use provider::{DESCRIBE_EVERY, HyperionProvider, KEEPALIVE};
pub use settings::{HyperionConfig, WEB_UI_PORT};
