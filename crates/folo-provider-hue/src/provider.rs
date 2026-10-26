//! The Hue kind and its instances (`docs/architecture/providers.md`, ADR-0013).
//!
//! An instance keeps a control connection (REST, authenticated with the application key) and,
//! while a target is driven, one DTLS stream to one entertainment configuration. A bridge has
//! one stream, so the targets of an instance that are driven at once must belong to the same
//! configuration. Managed configurations (RB-13 step 7) arrive later: until then the targets
//! are the lights of the bridge's existing configurations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use folo_protocol::generated::{
    DiscoveredDevice, HueSettings, ProviderInstance, ProviderInstanceKind, ProviderSecrets,
    ProviderSettings,
};
use folo_provider::{
    AdminError, AdminMethod, BoxFuture, DeliveryPolicy, DescribedTarget, ElementOutput, Provider,
    ProviderError, ProviderKind,
};
use serde_json::Value;

use crate::bridge::{BridgeInfo, client_for};
use crate::discovery::{DiscoveryConfig, discover};
use crate::dtls::DtlsSettings;
use crate::endpoint::Endpoint;
use crate::model::{HueTarget, Inventory};
use crate::rest::{BridgeClient, RestError, RestLimits};
use crate::stream::ActiveStream;
use crate::trust::{Anchors, BridgeTrust, Fingerprint, MemoryPins, PinStore};

/// What a node can choose about Hue; the defaults are a real bridge's.
#[derive(Clone)]
pub struct HueOptions {
    pub anchors: Anchors,
    pub rest: RestLimits,
    /// The bridge's UDP port.
    pub dtls_port: u16,
    pub dtls_attempts: u32,
    pub dtls_retransmit_min: Duration,
    pub dtls_retransmit_max: Duration,
    /// How often and how far apart `start` and `stop` are tried.
    pub action_tries: u32,
    pub action_pause: Duration,
    pub discovery: DiscoveryConfig,
    /// Certificates people confirmed.
    pub pins: Arc<dyn PinStore>,
}

impl Default for HueOptions {
    fn default() -> Self {
        let dtls = DtlsSettings::new("", Vec::new());
        Self {
            anchors: Anchors::Signify,
            rest: RestLimits::default(),
            dtls_port: Endpoint::DTLS_PORT,
            dtls_attempts: dtls.attempts,
            dtls_retransmit_min: dtls.retransmit_min,
            dtls_retransmit_max: dtls.retransmit_max,
            action_tries: 3,
            action_pause: Duration::from_millis(500),
            discovery: DiscoveryConfig::default(),
            pins: Arc::new(MemoryPins::default()),
        }
    }
}

/// The Hue kind: discovery, and making instances.
pub struct HueKind {
    options: HueOptions,
}

impl Default for HueKind {
    fn default() -> Self {
        Self::new(HueOptions::default())
    }
}

impl HueKind {
    #[must_use]
    pub fn new(options: HueOptions) -> Self {
        Self { options }
    }

    /// Remembers that a person confirmed this certificate for this bridge (trust on first use).
    /// The node keeps the pin with the instance and puts it back through [`HueOptions::pins`].
    pub fn pin_certificate(&self, bridge_id: &str, fingerprint: Fingerprint) {
        self.options.pins.pin(bridge_id, fingerprint);
    }
}

impl ProviderKind for HueKind {
    fn kind(&self) -> ProviderInstanceKind {
        ProviderInstanceKind::Hue
    }

    fn discover(&self) -> BoxFuture<'_, Result<Vec<DiscoveredDevice>, ProviderError>> {
        Box::pin(async move {
            discover(&self.options.discovery)
                .await
                .iter()
                .map(crate::discovery::Candidate::device)
                .collect()
        })
    }

    fn create(
        &self,
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Arc<dyn Provider>, ProviderError> {
        let ProviderSettings::HueSettings(settings) = &instance.settings else {
            return Err(ProviderError::Failed(
                "the instance has no Hue settings".into(),
            ));
        };
        Ok(Arc::new(HueProvider::new(
            &self.options,
            settings,
            secrets,
        )?))
    }
}

/// A connected bridge.
#[derive(Clone)]
pub(crate) struct Connection {
    pub client: BridgeClient,
    /// What the bridge calls this application; `None` when it didn't say.
    pub application_id: Option<String>,
    /// The DTLS identity: the application id, else the application key.
    pub identity: String,
}

/// One bridge.
pub struct HueProvider {
    pub(crate) options: HueOptions,
    pub(crate) endpoint: Endpoint,
    pub(crate) bridge_id: String,
    app_key: Option<String>,
    client_key: Option<String>,
    pub(crate) connection: Mutex<Option<Connection>>,
    pub(crate) targets: Mutex<HashMap<String, HueTarget>>,
    pub(crate) stream: tokio::sync::Mutex<Option<ActiveStream>>,
    /// A configuration whose stream may be running on the bridge without `stream` knowing: a
    /// `start` in progress, or a stream that failed. The next release or connection stops it.
    pub(crate) pending_stop: Mutex<Option<String>>,
}

impl HueProvider {
    /// # Errors
    /// When the instance names no bridge or its address can't be used.
    pub fn new(
        options: &HueOptions,
        settings: &HueSettings,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Self, ProviderError> {
        if settings.bridge_id.is_empty() {
            return Err(ProviderError::Failed(
                "the instance has no bridge id: choose a bridge from discovery".into(),
            ));
        }
        let mut endpoint = Endpoint::parse(&settings.address)?;
        endpoint.dtls_port = options.dtls_port;
        Ok(Self {
            options: options.clone(),
            endpoint,
            bridge_id: (*settings.bridge_id).clone(),
            app_key: secrets
                .and_then(|s| s.app_key.as_ref())
                .map(|k| (**k).clone()),
            client_key: secrets
                .and_then(|s| s.client_key.as_ref())
                .map(|k| (**k).clone()),
            connection: Mutex::new(None),
            targets: Mutex::new(HashMap::new()),
            stream: tokio::sync::Mutex::new(None),
            pending_stop: Mutex::new(None),
        })
    }

    fn trust(&self) -> BridgeTrust {
        BridgeTrust {
            bridge_id: self.bridge_id.clone(),
            anchors: self.options.anchors.clone(),
            pinned: self.options.pins.pinned(&self.bridge_id),
        }
    }

    pub(crate) fn connected(&self) -> Result<Connection, ProviderError> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| ProviderError::Unreachable("not connected to the bridge".into()))
    }

    pub(crate) fn dtls_settings(&self, identity: &str) -> Result<DtlsSettings, ProviderError> {
        let key = self.client_key.as_deref().ok_or_else(|| {
            ProviderError::Refused("the bridge has no client key: pair again".into())
        })?;
        let key = hex::decode(key)
            .map_err(|_| ProviderError::Refused("the client key isn't hex: pair again".into()))?;
        let mut settings = DtlsSettings::new(identity, key);
        settings.attempts = self.options.dtls_attempts;
        settings.retransmit_min = self.options.dtls_retransmit_min;
        settings.retransmit_max = self.options.dtls_retransmit_max;
        Ok(settings)
    }

    async fn open(&self) -> Result<Connection, ProviderError> {
        let app_key = self.app_key.clone().ok_or_else(|| {
            ProviderError::Refused("not paired with the bridge: press its link button".into())
        })?;
        let client = client_for(
            &self.endpoint,
            self.trust(),
            Some(app_key.clone()),
            self.options.rest,
        )?;
        let info = BridgeInfo::from_config(&client.public_config().await?)?;
        if !info.bridge_id.eq_ignore_ascii_case(&self.bridge_id) {
            return Err(ProviderError::Refused(format!(
                "the device at {} is bridge {}, not {}",
                self.endpoint.host, info.bridge_id, self.bridge_id
            )));
        }
        info.check_supported()?;
        let application_id = match client.application_id().await {
            Ok(id) => Some(id),
            // The identity falls back to the key when the bridge names none
            Err(RestError::Malformed(_)) => None,
            Err(RestError::Unauthorized) => {
                return Err(ProviderError::Refused(
                    "the bridge doesn't accept the application key: pair again".into(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        let identity = application_id.clone().unwrap_or(app_key);
        Ok(Connection {
            client,
            application_id,
            identity,
        })
    }

    /// Reads the bridge's lights and areas.
    pub(crate) async fn inventory(
        &self,
        client: &BridgeClient,
    ) -> Result<Inventory, ProviderError> {
        let (configurations, services, lights, devices) = tokio::try_join!(
            client.resources("entertainment_configuration"),
            client.resources("entertainment"),
            client.resources("light"),
            client.resources("device"),
        )?;
        Ok(Inventory::from_resources(
            configurations,
            services,
            lights,
            devices,
        ))
    }

    /// Reads the targets again and keeps them: `start` and `send` look them up.
    pub(crate) async fn refresh_targets(
        &self,
        client: &BridgeClient,
    ) -> Result<Vec<HueTarget>, ProviderError> {
        let targets = self.inventory(client).await?.targets();
        let mut known = self.targets.lock().unwrap_or_else(PoisonError::into_inner);
        *known = targets.iter().map(|t| (t.id.clone(), t.clone())).collect();
        Ok(targets)
    }
}

impl Provider for HueProvider {
    fn connect(&self) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(async move {
            // A stream left over from before the connection was lost belongs to no one now
            self.discard_stream().await;
            let connection = self.open().await?;
            *self
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(connection.clone());
            self.stop_pending(&connection).await;
            Ok(())
        })
    }

    fn describe(&self) -> BoxFuture<'_, Result<Vec<DescribedTarget>, ProviderError>> {
        Box::pin(async move {
            let connection = self.connected()?;
            let mut targets = self.refresh_targets(&connection.client).await?;
            targets.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(targets.iter().map(HueTarget::described).collect())
        })
    }

    fn disconnect(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Ok(connection) = self.connected() {
                self.end_stream(&connection).await;
            } else {
                self.discard_stream().await;
            }
            *self
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
        })
    }

    fn admin(
        &self,
        action: &str,
        method: AdminMethod,
        body: Value,
    ) -> BoxFuture<'_, Result<Value, AdminError>> {
        let action = action.to_owned();
        Box::pin(async move { self.administer(&action, method, body).await })
    }

    fn delivery_policy(&self) -> DeliveryPolicy {
        DeliveryPolicy {
            // The bridge closes the stream after ten seconds without data
            keepalive: Duration::from_secs(5),
            // Starting is the slow call: the area's start, then the handshake
            call_timeout: Duration::from_secs(40),
            // A release is close_notify, then the area's stop
            release_timeout: Duration::from_secs(4),
            ..DeliveryPolicy::default()
        }
    }

    fn element_output(&self) -> Option<&dyn ElementOutput> {
        Some(self)
    }
}
