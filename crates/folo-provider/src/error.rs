//! What a provider's operations fail with.

use std::fmt;

/// A provider operation failed. The message is for the log and the live channel's `error` state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    /// The device didn't answer or the connection closed.
    Unreachable(String),
    /// The device refused: credentials, a pairing, a setting it doesn't accept.
    Refused(String),
    /// Anything else the provider can't carry on with.
    Failed(String),
}

impl ProviderError {
    /// The message alone.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Unreachable(m) | Self::Refused(m) | Self::Failed(m) => m,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(m) => write!(f, "unreachable: {m}"),
            Self::Refused(m) => write!(f, "refused: {m}"),
            Self::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ProviderError {}

/// An administration action failed. [`status`](Self::status) and [`code`](Self::code) are what
/// the client API answers (`docs/specs/client-api.md`, providers).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdminError {
    /// The kind has no implementation yet: `501 not_implemented`, naming the run book.
    NotImplemented { runbook: &'static str },
    /// The kind has no such action: `404 not_found`.
    UnknownAction(String),
    /// The request's body isn't what the action takes: `400 bad_body`.
    BadRequest(String),
    /// The device refused or didn't answer: `502 provider_error`.
    Provider(ProviderError),
}

impl AdminError {
    /// The HTTP status.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::NotImplemented { .. } => 501,
            Self::UnknownAction(_) => 404,
            Self::BadRequest(_) => 400,
            Self::Provider(_) => 502,
        }
    }

    /// The error body's machine code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented { .. } => "not_implemented",
            Self::UnknownAction(_) => "not_found",
            Self::BadRequest(_) => "bad_body",
            Self::Provider(_) => "provider_error",
        }
    }
}

impl fmt::Display for AdminError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented { runbook } => write!(f, "arrives with {runbook}"),
            Self::UnknownAction(a) => write!(f, "no such action: {a}"),
            Self::BadRequest(why) => write!(f, "bad request: {why}"),
            Self::Provider(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AdminError {}

impl From<ProviderError> for AdminError {
    fn from(value: ProviderError) -> Self {
        Self::Provider(value)
    }
}
