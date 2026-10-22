//! The FOLO node (ADR-0003, RB-06): one binary, `folo`, with the controller
//! and source roles. This core reads its settings and documents, keeps its
//! identity, serves its port's loopback routes, supervises the effect engine
//! and reports its status.

pub mod cli;
pub mod client;
pub mod import;
pub mod lock;
pub mod logs;
pub mod node;
pub mod server;
pub mod status;
