//! Knowing a bridge: what it says of itself, whether FOLO can use it, and whether to believe it.

use std::sync::{Arc, Mutex, PoisonError};

use folo_provider::ProviderError;
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::rest::{BridgeClient, RestLimits};
use crate::trust::{
    Anchors, BridgeTrust, CertificateSummary, TrustError, bridge_config, capture_config, summarize,
};

/// The bridge models FOLO knows: the square bridge (v2) and the Bridge Pro.
pub const SUPPORTED_MODELS: [&str; 2] = ["BSB002", "BSB003"];

/// The lowest `apiversion` that has entertainment streaming through CLIP v2.
pub const MINIMUM_API_VERSION: (u32, u32) = (1, 22);

/// What an unauthenticated `GET /api/config` says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeInfo {
    pub name: String,
    pub bridge_id: String,
    pub model_id: String,
    pub software_version: String,
    pub api_version: String,
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

impl BridgeInfo {
    /// Reads the answer of `/api/config`.
    ///
    /// # Errors
    /// When it names no bridge id.
    pub fn from_config(config: &Value) -> Result<Self, ProviderError> {
        let info = Self {
            name: text(config, "name"),
            bridge_id: text(config, "bridgeid"),
            model_id: text(config, "modelid"),
            software_version: text(config, "swversion"),
            api_version: text(config, "apiversion"),
        };
        if info.bridge_id.is_empty() {
            return Err(ProviderError::Failed(
                "that is not a Hue bridge: it names no bridge id".into(),
            ));
        }
        Ok(info)
    }

    /// Whether FOLO can stream to it: a known model and `apiversion` 1.22 or later.
    ///
    /// # Errors
    /// [`ProviderError::Refused`] saying which check failed.
    pub fn check_supported(&self) -> Result<(), ProviderError> {
        if !SUPPORTED_MODELS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&self.model_id))
        {
            return Err(ProviderError::Refused(format!(
                "bridge model {:?} isn't supported (FOLO knows {})",
                self.model_id,
                SUPPORTED_MODELS.join(" and ")
            )));
        }
        match parse_api_version(&self.api_version) {
            Some(version) if version >= MINIMUM_API_VERSION => Ok(()),
            _ => Err(ProviderError::Refused(format!(
                "the bridge's API version {:?} is older than {}.{} and can't stream",
                self.api_version, MINIMUM_API_VERSION.0, MINIMUM_API_VERSION.1
            ))),
        }
    }
}

/// `"1.65.0"` as (1, 65).
fn parse_api_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether a probe believed the bridge's certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeTrust {
    /// It chains to a Signify root and is named for the bridge.
    Trusted,
    /// It is named for the bridge but doesn't chain to a Signify root (older firmware). It may be
    /// trusted after a person confirmed its fingerprint.
    NeedsConfirmation(TrustError),
    /// It is for another name, or unreadable. Never offered for confirmation.
    Rejected(TrustError),
}

/// What a bridge answered when asked who it is, and what to make of its certificate.
#[derive(Clone, Debug)]
pub struct Probe {
    pub info: BridgeInfo,
    pub certificate: CertificateSummary,
    pub trust: ProbeTrust,
}

/// A REST client that checks the bridge with `trust`.
///
/// # Errors
/// [`ProviderError::Failed`] when the TLS settings or the client can't be built.
pub fn client_for(
    endpoint: &Endpoint,
    trust: BridgeTrust,
    app_key: Option<String>,
    limits: RestLimits,
) -> Result<BridgeClient, ProviderError> {
    let tls = bridge_config(trust).map_err(|e| ProviderError::Failed(e.to_string()))?;
    BridgeClient::new(endpoint.clone(), tls, app_key, limits)
}

/// Asks the bridge at `endpoint` who it is, without believing anything: `/api/config` holds
/// nothing secret, and no credentials are sent. The certificate it presented is then judged
/// against the bridge id it gave.
///
/// # Errors
/// When the bridge can't be reached or isn't a bridge.
pub async fn probe(
    endpoint: &Endpoint,
    anchors: &Anchors,
    limits: RestLimits,
) -> Result<Probe, ProviderError> {
    let seen = Arc::new(Mutex::new(None));
    let tls =
        capture_config(Arc::clone(&seen)).map_err(|e| ProviderError::Failed(e.to_string()))?;
    let client = BridgeClient::new(endpoint.clone(), tls, None, limits)?;
    let info = BridgeInfo::from_config(&client.public_config().await?)?;
    let mut chain = seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .ok_or_else(|| ProviderError::Failed("the bridge presented no certificate".into()))?;
    let der = chain.remove(0);
    let certificate = summarize(&der).map_err(|e| ProviderError::Failed(e.to_string()))?;
    let judged = BridgeTrust {
        bridge_id: info.bridge_id.clone(),
        anchors: anchors.clone(),
        pinned: None,
    };
    let trust = match judged.check(&der, &chain) {
        Ok(()) => ProbeTrust::Trusted,
        Err(e @ TrustError::Chain(_)) => ProbeTrust::NeedsConfirmation(e),
        Err(e) => ProbeTrust::Rejected(e),
    };
    Ok(Probe {
        info,
        certificate,
        trust,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn info(model: &str, api: &str) -> BridgeInfo {
        BridgeInfo::from_config(&json!({
            "name": "Hue Bridge", "bridgeid": "001788FFFE123456", "modelid": model,
            "swversion": "1976154040", "apiversion": api,
        }))
        .unwrap()
    }

    #[test]
    fn models_and_api_versions_are_checked() {
        assert!(info("BSB002", "1.65.0").check_supported().is_ok());
        assert!(info("BSB003", "1.22.0").check_supported().is_ok());
        assert!(info("bsb002", "2.0.0").check_supported().is_ok());
        assert!(info("BSB002", "1.21.9").check_supported().is_err());
        assert!(info("BSB001", "1.65.0").check_supported().is_err());
        assert!(info("BSB002", "").check_supported().is_err());
        assert!(BridgeInfo::from_config(&json!({"name": "x"})).is_err());
    }
}
