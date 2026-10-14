//! FOLO's contracts in Rust (RB-02): the types generated from `specs/schema`,
//! the clampers, and the version-1 config rules.

pub mod clamp;
pub mod number;
pub mod v1;

/// Types generated from the JSON Schemas by typify (`scripts/contracts.py generate`).
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, unused, missing_docs)]
pub mod generated;
