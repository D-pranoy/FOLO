//! The WLED provider (RB-12, ADR-0012, `docs/integrations/wled.md`).
//!
//! - [`WledKind`]: discovery (mDNS `_wled._tcp`, and [`probe`] for manual entry) and making
//!   instances
//! - [`WledProvider`]: connect, describe (the strip, its segments, a matrix, PWM and relay
//!   outputs), realtime output over DDP or DNRGB(W), release, and basic administration
//! - [`testing`]: a fake device (its JSON API over HTTP, its UDP realtime receiver) and fixtures
//!   of devices, for these tests and the node's
//!
//! Gamma is not applied here: FOLO's adapter chain does it before the frame reaches a provider.
//! Nothing here is registered in a node; the node's controller output wires kinds in.

mod admin;
mod api;
pub mod ddp;
mod describe;
mod device;
mod discovery;
pub mod dnrgb;
pub mod http;
mod pixels;
mod provider;
mod reader;
mod settings;
mod sink;

pub mod testing;

pub use api::{DeviceApi, HttpApi};
pub use describe::{Description, WledTarget};
pub use device::{Bus, BusModel, Device, RealtimeSettings, Segment};
pub use discovery::{Browse, MdnsBrowser, ServiceAnswer, device_from_answer, instance_name, probe};
pub use pixels::pixel_bytes;
pub use provider::{WledKind, WledProvider};
pub use reader::{read_cfg, read_device};
pub use settings::{Config, Transport};
pub use sink::{DatagramSink, UdpSink};
