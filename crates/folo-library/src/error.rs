//! What the library refuses, with the status, code and details the client
//! API answers (docs/specs/client-api.md#library).

use serde_json::{Value, json};

/// Why a library operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryError {
    /// `422 structural_error`: the pack, as a whole, isn't a pack.
    Structural { path: String, reason: String },
    /// `422 incompatible`: its `folo` range excludes this node.
    Incompatible { folo: String },
    /// `422 missing_dependency`: `requires` entries not installed in a matching version.
    MissingDependency { requires: Vec<(String, String)> },
    /// `409 reserved_id`: the author `folo` or `local`.
    ReservedId { id: String },
    /// `409 already_installed`.
    AlreadyInstalled { installed: String, offered: String },
    /// `409 library_full`.
    LibraryFull { reason: String },
    /// `404 not_found`.
    NotFound { what: String },
    /// `409 protected_pack`.
    ProtectedPack { id: String },
    /// `409 required_by`.
    RequiredBy { packs: Vec<String> },
    /// `409 read_only_pack`.
    ReadOnlyPack { id: String },
    /// `409 pack_unreadable`.
    PackUnreadable { id: String },
    /// `409 id_taken`: the id is another type's item.
    IdTaken { id: String, type_name: String },
    /// `409 list_full`.
    ListFull { reason: String },
    /// `400 bad_body`.
    BadBody { reason: String },
    /// `500 internal`: the library's folder can't be written.
    Io { reason: String },
}

impl LibraryError {
    /// A structural error at `path`.
    #[must_use]
    pub fn structural(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Structural {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// The HTTP status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::Structural { .. }
            | Self::Incompatible { .. }
            | Self::MissingDependency { .. } => 422,
            Self::NotFound { .. } => 404,
            Self::BadBody { .. } => 400,
            Self::Io { .. } => 500,
            _ => 409,
        }
    }

    /// The error code in the API's error format.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Structural { .. } => "structural_error",
            Self::Incompatible { .. } => "incompatible",
            Self::MissingDependency { .. } => "missing_dependency",
            Self::ReservedId { .. } => "reserved_id",
            Self::AlreadyInstalled { .. } => "already_installed",
            Self::LibraryFull { .. } => "library_full",
            Self::NotFound { .. } => "not_found",
            Self::ProtectedPack { .. } => "protected_pack",
            Self::RequiredBy { .. } => "required_by",
            Self::ReadOnlyPack { .. } => "read_only_pack",
            Self::PackUnreadable { .. } => "pack_unreadable",
            Self::IdTaken { .. } => "id_taken",
            Self::ListFull { .. } => "list_full",
            Self::BadBody { .. } => "bad_body",
            Self::Io { .. } => "internal",
        }
    }

    /// The error's `details`.
    #[must_use]
    pub fn details(&self) -> Value {
        match self {
            Self::Structural { path, reason } => json!({"path": path, "reason": reason}),
            Self::Incompatible { folo } => json!({"folo": folo}),
            Self::MissingDependency { requires } => {
                let map: serde_json::Map<String, Value> = requires
                    .iter()
                    .map(|(id, range)| (id.clone(), Value::String(range.clone())))
                    .collect();
                json!({"requires": map})
            }
            Self::AlreadyInstalled { installed, offered } => {
                json!({"installed": installed, "offered": offered})
            }
            Self::RequiredBy { packs } => json!({"packs": packs}),
            _ => json!({}),
        }
    }
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Structural { path, reason } if path.is_empty() => write!(f, "{reason}"),
            Self::Structural { path, reason } => write!(f, "{path}: {reason}"),
            Self::Incompatible { folo } => write!(f, "the pack needs FOLO {folo}"),
            Self::MissingDependency { requires } => {
                let list: Vec<String> =
                    requires.iter().map(|(id, r)| format!("{id} {r}")).collect();
                write!(f, "the pack needs {}", list.join(", "))
            }
            Self::ReservedId { id } => write!(f, "{id}: the authors folo and local are reserved"),
            Self::AlreadyInstalled { installed, offered } => write!(
                f,
                "version {installed} is installed; {offered} isn't newer (replace=true installs it anyway)"
            ),
            Self::LibraryFull { reason } => write!(f, "the library is full: {reason}"),
            Self::NotFound { what } => write!(f, "{what} doesn't exist"),
            Self::ProtectedPack { id } => write!(f, "{id} can't be removed"),
            Self::RequiredBy { packs } => write!(f, "required by {}", packs.join(", ")),
            Self::ReadOnlyPack { id } => {
                write!(f, "{id} is read-only: only local packs are editable")
            }
            Self::PackUnreadable { id } => {
                write!(f, "{id} failed its checks at start and is left as it is")
            }
            Self::IdTaken { id, type_name } => {
                write!(f, "{id} is already an item of type {type_name}")
            }
            Self::ListFull { reason } => write!(f, "the pack is full: {reason}"),
            Self::BadBody { reason } => write!(f, "{reason}"),
            Self::Io { reason } => write!(f, "the library's folder can't be written: {reason}"),
        }
    }
}

impl std::error::Error for LibraryError {}

impl From<std::io::Error> for LibraryError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            reason: error.to_string(),
        }
    }
}
