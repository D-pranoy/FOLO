//! What the portal tests share: a private desktop (bus, PipeWire and its
//! session manager), a fake ScreenCast portal on that bus, and a test
//! producer that plays the compositor's screen node.

pub mod daemons;
pub mod fake_portal;
pub mod producer;
