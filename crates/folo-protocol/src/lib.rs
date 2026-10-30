//! FOLO's contracts in Rust (RB-02): the types generated from `specs/schema`,
//! the clampers, the version-1 config rules, and the source link's frames.

pub mod catalog;
pub mod clamp;
pub mod frames;
pub mod number;
pub mod v1;

/// Types generated from the JSON Schemas by typify (`scripts/contracts.py generate`).
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, unused, missing_docs)]
pub mod generated;
