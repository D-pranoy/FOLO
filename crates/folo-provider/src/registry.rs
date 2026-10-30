//! The provider kinds a node has.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use std::time::Duration;

use folo_protocol::generated::{
    DiscoveredDevice, HueSettings, HyperionSettings, ProviderInstance, ProviderInstanceKind,
    ProviderSecrets, ProviderSettings, WledSettings,
};
use serde_json::Value;

use crate::error::ProviderError;
use crate::traits::{Provider, ProviderKind};

/// How long one kind's discovery may take before it counts as having found nothing.
pub const DISCOVER_TIMEOUT: Duration = Duration::from_secs(10);

/// An instance from its stored JSON, with its `settings` read as its `kind`'s.
///
/// The generated `ProviderSettings` is an untagged union whose `kind` is free-form, so serde
/// takes the first variant that fits: WLED settings without `transport` and `timeout_s` would
/// read as Hyperion's. The instance's own `kind` decides instead.
///
/// # Errors
/// When the JSON isn't an instance, or its settings aren't its kind's.
pub fn instance_of(value: &Value) -> Result<ProviderInstance, serde_json::Error> {
    let mut value = value.clone();
    let kind = value.get("kind").cloned().unwrap_or(Value::Null);
    // settings left out are the kind's defaults
    if let Some(fields) = value.as_object_mut() {
        let settings = fields
            .entry("settings")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(settings) = settings.as_object_mut() {
            settings.entry("kind").or_insert(kind);
        }
    }
    let mut instance: ProviderInstance = serde_json::from_value(value.clone())?;
    let settings = value["settings"].clone();
    instance.settings = match instance.kind {
        ProviderInstanceKind::Hyperion => ProviderSettings::HyperionSettings(
            serde_json::from_value::<HyperionSettings>(settings)?,
        ),
        ProviderInstanceKind::Wled => {
            ProviderSettings::WledSettings(serde_json::from_value::<WledSettings>(settings)?)
        }
        ProviderInstanceKind::Hue => {
            ProviderSettings::HueSettings(serde_json::from_value::<HueSettings>(settings)?)
        }
    };
    Ok(instance)
}

/// The run book that brings a kind's provider.
#[must_use]
pub fn runbook(kind: ProviderInstanceKind) -> &'static str {
    match kind {
        ProviderInstanceKind::Hyperion => "RB-10",
        ProviderInstanceKind::Wled => "RB-12",
        ProviderInstanceKind::Hue => "RB-13",
    }
}

/// What the live channel says of an instance whose kind has no implementation yet.
#[must_use]
pub fn not_implemented_message(kind: ProviderInstanceKind) -> String {
    format!("arrives with {}", runbook(kind))
}

/// Why an instance couldn't be made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateError {
    /// The kind has no implementation in this node.
    NotImplemented {
        kind: ProviderInstanceKind,
        runbook: &'static str,
    },
    /// The kind refused the instance's settings.
    Provider(ProviderError),
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented { kind, .. } => f.write_str(&not_implemented_message(*kind)),
            Self::Provider(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CreateError {}

/// The kinds a node has. A node registers the kinds it implements; the others are absent, and
/// their instances exist as configuration only.
#[derive(Default)]
pub struct Registry {
    kinds: BTreeMap<ProviderInstanceKind, Arc<dyn ProviderKind>>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a kind, replacing one registered for the same kind before.
    pub fn register(&mut self, kind: Arc<dyn ProviderKind>) {
        self.kinds.insert(kind.kind(), kind);
    }

    #[must_use]
    pub fn get(&self, kind: ProviderInstanceKind) -> Option<&Arc<dyn ProviderKind>> {
        self.kinds.get(&kind)
    }

    #[must_use]
    pub fn is_implemented(&self, kind: ProviderInstanceKind) -> bool {
        self.kinds.contains_key(&kind)
    }

    /// The registered kinds, in a fixed order.
    pub fn kinds(&self) -> impl Iterator<Item = ProviderInstanceKind> + '_ {
        self.kinds.keys().copied()
    }

    /// Discovery over the registered kinds among `kinds`, one after the other, gathered into one
    /// list. A kind that isn't registered adds nothing (so the list is empty until the first
    /// provider arrives), and one that fails, or takes longer than [`DISCOVER_TIMEOUT`], is
    /// logged and adds nothing.
    pub async fn discover(&self, kinds: &[ProviderInstanceKind]) -> Vec<DiscoveredDevice> {
        let mut found = Vec::new();
        for (kind, provider) in &self.kinds {
            if !kinds.contains(kind) {
                continue;
            }
            match tokio::time::timeout(DISCOVER_TIMEOUT, provider.discover()).await {
                Ok(Ok(mut devices)) => found.append(&mut devices),
                Ok(Err(e)) => log::warn!("discovering {kind} devices failed: {e}"),
                Err(_) => log::warn!(
                    "discovering {kind} devices took longer than {} s",
                    DISCOVER_TIMEOUT.as_secs()
                ),
            }
        }
        found
    }

    /// Makes the provider of an instance.
    ///
    /// # Errors
    /// [`CreateError::NotImplemented`] when the instance's kind isn't registered.
    pub fn create(
        &self,
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Arc<dyn Provider>, CreateError> {
        let kind = instance.kind;
        let Some(provider) = self.kinds.get(&kind) else {
            return Err(CreateError::NotImplemented {
                kind,
                runbook: runbook(kind),
            });
        };
        provider
            .create(instance, secrets)
            .map_err(CreateError::Provider)
    }
}
