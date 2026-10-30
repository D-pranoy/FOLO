//! The Philips Hue provider (RB-13, ADR-0013, `docs/integrations/hue.md`).
//!
//! - [`HueKind`] is the [`folo_provider::ProviderKind`] a node registers: discovery (mDNS, then
//!   Signify's discovery service) and making [`HueProvider`] instances.
//! - [`HueProvider`] keeps a bridge's control connection (HTTPS, application key), describes its
//!   lights and entertainment areas as point and line targets, streams to an area over DTLS, and
//!   offers the basic administration of ADR-0014.
//! - [`bridge::probe`] asks an address who it is and whether to believe its certificate, and
//!   [`pairing::pair`] runs the link-button pairing. Their results are for the node to keep:
//!   the credentials in the secrets store, a confirmed certificate through [`trust::PinStore`].
//!
//! Nothing here talks to anything but the bridge it is given.

mod admin;
pub mod bridge;
pub mod discovery;
pub mod dtls;
pub mod encoder;
pub mod endpoint;
pub mod model;
pub mod pairing;
mod provider;
pub mod rest;
mod stream;
pub mod trust;

pub use provider::{HueKind, HueOptions, HueProvider};
pub use stream::IN_USE_BY_ANOTHER_APP;
