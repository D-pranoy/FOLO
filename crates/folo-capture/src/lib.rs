//! The display capture agent (RB-14, ADR-0037): the binary `folo-capture`.
//! It runs in the signed-in user's session, is started by its node with the
//! capture link's URL and launch token in its environment, and captures one
//! screen on demand. Safe code only (D19): the backends that call into the
//! operating system are crates of their own.

pub mod backends;
pub mod cli;
pub mod environment;
pub mod link;
pub mod logs;
pub mod worker;
