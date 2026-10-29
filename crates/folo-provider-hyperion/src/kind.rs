//! The Hyperion kind: what the node registers for `hyperion` instances.

use std::sync::Arc;

use folo_protocol::generated::{
    DiscoveredDevice, ProviderInstance, ProviderInstanceKind, ProviderSecrets,
};
use folo_provider::{BoxFuture, Provider, ProviderError, ProviderKind};

use crate::provider::HyperionProvider;
use crate::settings::HyperionConfig;

pub struct HyperionKind;

impl ProviderKind for HyperionKind {
    fn kind(&self) -> ProviderInstanceKind {
        ProviderInstanceKind::Hyperion
    }

    /// Finds nothing: Hyperion setups are entered by address, and network discovery isn't part
    /// of RB-10.
    fn discover(&self) -> BoxFuture<'_, Result<Vec<DiscoveredDevice>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn create(
        &self,
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Arc<dyn Provider>, ProviderError> {
        let config = HyperionConfig::from_instance(instance, secrets)?;
        Ok(Arc::new(HyperionProvider::new(config)))
    }
}
