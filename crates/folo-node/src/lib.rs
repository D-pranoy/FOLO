//! The FOLO node (ADR-0003, RB-06, RB-07): one binary, `folo`, with the
//! controller and source roles. It reads its settings and documents, keeps
//! its identity, serves the client API, the live channel and the UI on its
//! port, supervises the effect engine and reports its status.

pub mod api;
pub mod cli;
pub mod client;
pub mod import;
pub mod library_api;
pub mod live;
pub mod lock;
pub mod logs;
pub mod node;
pub mod server;
pub mod status;
pub mod ui;
