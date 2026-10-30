//! The content library (RB-09 step 1, ADR-0038 D1 to D10): the built-in
//! packs embedded from `packs/`, installed and local packs in the data
//! folder's `library/`, install and export of `.folopack` archives, and the
//! item hash over canonical JSON (RFC 8785).

pub mod archive;
pub mod bind;
pub mod builtin;
pub mod canonical;
pub mod compat;
pub mod copy;
pub mod disk;
mod edit;
pub mod error;
mod library;
pub mod limits;
pub mod merge;
pub mod names;
pub mod pack;
pub mod reference;
pub mod scale;

pub use error::LibraryError;
pub use library::{Library, LibraryEvent, MY_PRESETS, SceneItem, State};
pub use names::ItemType;
