//! An instance's settings and credentials, as the provider uses them.

use folo_protocol::generated::{ProviderInstance, ProviderSecrets, ProviderSettings};
use folo_provider::ProviderError;

use crate::wire::{MAX_ORIGIN_CHARS, MIN_ORIGIN_CHARS};

/// Where Hyperion's own web UI listens unless its owner moved it (`serverinfo` doesn't say).
pub const WEB_UI_PORT: u16 = 8090;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperionConfig {
    pub host: String,
    pub port: u16,
    /// The priority FOLO's colours have, and the one it clears.
    pub priority: u16,
    /// At most 20 characters: Hyperion rejects longer ones.
    pub origin: String,
    /// The API token, for a Hyperion with authentication on.
    pub token: Option<String>,
}

impl HyperionConfig {
    /// Reads the settings of a Hyperion instance and its token.
    ///
    /// # Errors
    /// [`ProviderError::Failed`] when the instance isn't a Hyperion one or a setting can't be
    /// used (the documents are clamped, so this is a guard, not a validation).
    pub fn from_instance(
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Self, ProviderError> {
        let ProviderSettings::HyperionSettings(settings) = &instance.settings else {
            return Err(ProviderError::Failed(format!(
                "instance {} has no Hyperion settings",
                *instance.id
            )));
        };
        let origin = (*settings.origin).clone();
        // Hyperion's color command takes 4 to 20 characters, and the documents only cap them
        if !(MIN_ORIGIN_CHARS..=MAX_ORIGIN_CHARS).contains(&origin.chars().count()) {
            return Err(ProviderError::Failed(format!(
                "the origin must have {MIN_ORIGIN_CHARS} to {MAX_ORIGIN_CHARS} characters"
            )));
        }
        let too_big = |what: &str| ProviderError::Failed(format!("the {what} is out of range"));
        Ok(Self {
            host: (*settings.host).clone(),
            port: u16::try_from(settings.port.get()).map_err(|_| too_big("port"))?,
            priority: u16::try_from(settings.priority.get()).map_err(|_| too_big("priority"))?,
            origin,
            token: secrets
                .and_then(|s| s.token.as_ref())
                .map(|t| (**t).clone())
                .filter(|t| !t.is_empty()),
        })
    }
}
