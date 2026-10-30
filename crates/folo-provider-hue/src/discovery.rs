//! Finding bridges (`docs/integrations/hue.md`, discovery): mDNS `_hue._tcp.local.` first, then
//! Signify's discovery service, and manual entry (see [`crate::bridge::probe`]).

use std::collections::HashSet;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use folo_protocol::generated::{
    DiscoveredDevice, DiscoveredDeviceAddress, DiscoveredDeviceId, DiscoveredDeviceKind,
    DiscoveredDeviceName,
};
use folo_provider::ProviderError;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::trust::public_config;

/// How bridges are looked for.
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    /// The mDNS service type.
    pub mdns_service: String,
    /// How long to listen for mDNS answers.
    pub mdns_timeout: Duration,
    /// Signify's discovery service, asked when mDNS finds nothing. `None` turns it off, as tests
    /// do to stay off the internet.
    pub cloud_url: Option<String>,
    /// Certificates (DER) to trust for the discovery service besides the system's.
    pub cloud_roots: Vec<Vec<u8>>,
    /// How long the discovery service has to answer.
    pub cloud_timeout: Duration,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mdns_service: "_hue._tcp.local.".to_owned(),
            mdns_timeout: Duration::from_secs(3),
            cloud_url: Some("https://discovery.meethue.com/".to_owned()),
            cloud_roots: Vec::new(),
            cloud_timeout: Duration::from_secs(5),
        }
    }
}

/// A bridge as discovery names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub bridge_id: String,
    pub host: String,
    pub https_port: u16,
    pub name: String,
}

impl Candidate {
    /// As the client API lists it: the address is `host`, or `host:port` for another port.
    ///
    /// # Errors
    /// When a field is longer than the contract allows.
    pub fn device(&self) -> Result<DiscoveredDevice, ProviderError> {
        let address = if self.https_port == Endpoint::HTTPS_PORT {
            self.host.clone()
        } else if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.https_port)
        } else {
            format!("{}:{}", self.host, self.https_port)
        };
        let invalid = |e: String| ProviderError::Failed(format!("discovered a bridge with {e}"));
        Ok(DiscoveredDevice {
            address: address
                .parse::<DiscoveredDeviceAddress>()
                .map_err(|e| invalid(e.to_string()))?,
            id: self
                .bridge_id
                .parse::<DiscoveredDeviceId>()
                .map_err(|e| invalid(e.to_string()))?,
            kind: DiscoveredDeviceKind::Hue,
            name: self
                .name
                .parse::<DiscoveredDeviceName>()
                .map_err(|e| invalid(e.to_string()))?,
        })
    }
}

/// A bridge from an mDNS answer: the id is the `bridgeid` text record, the name the instance
/// name (`Philips Hue - 23BFC2`).
#[must_use]
pub fn candidate_from_service(info: &ResolvedService) -> Option<Candidate> {
    let bridge_id = info.get_property_val_str("bridgeid")?.to_owned();
    // An IPv4 address is what a bridge is reached on by default
    let mut addresses: Vec<IpAddr> = info
        .get_addresses()
        .iter()
        .map(mdns_sd::ScopedIp::to_ip_addr)
        .collect();
    addresses.sort_by_key(|a| (a.is_ipv6(), *a));
    let host = addresses.first()?.to_string();
    let name = info
        .get_fullname()
        .split("._")
        .next()
        .unwrap_or_default()
        .to_owned();
    Some(Candidate {
        bridge_id,
        host,
        https_port: info.port,
        name,
    })
}

/// The bridges that answer over mDNS within the configured time.
async fn mdns(config: &DiscoveryConfig) -> Result<Vec<Candidate>, ProviderError> {
    let service = config.mdns_service.clone();
    let timeout = config.mdns_timeout;
    tokio::task::spawn_blocking(move || browse(&service, timeout))
        .await
        .map_err(|e| ProviderError::Failed(format!("mDNS discovery stopped: {e}")))?
}

fn browse(service: &str, timeout: Duration) -> Result<Vec<Candidate>, ProviderError> {
    let failed = |e: mdns_sd::Error| ProviderError::Failed(format!("mDNS discovery failed: {e}"));
    let daemon = ServiceDaemon::new().map_err(failed)?;
    let receiver = daemon.browse(service).map_err(failed)?;
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(resolved)) => {
                found.extend(candidate_from_service(&resolved));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    if let Err(e) = daemon.shutdown() {
        log::debug!("stopping the mDNS daemon failed: {e}");
    }
    Ok(found)
}

/// The bridges Signify's discovery service knows on this network.
async fn cloud(config: &DiscoveryConfig, url: &str) -> Result<Vec<Candidate>, ProviderError> {
    let tls =
        public_config(&config.cloud_roots).map_err(|e| ProviderError::Failed(e.to_string()))?;
    let http = reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(config.cloud_timeout)
        .build()
        .map_err(|e| ProviderError::Failed(e.without_url().to_string()))?;
    let answer: Value = http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| ProviderError::Unreachable(e.without_url().to_string()))?
        .json()
        .await
        .map_err(|e| ProviderError::Failed(e.without_url().to_string()))?;
    Ok(candidates_from_cloud(&answer))
}

/// The entries of the discovery service's answer: `[{id, internalipaddress, port}]`.
#[must_use]
pub fn candidates_from_cloud(answer: &Value) -> Vec<Candidate> {
    let Some(list) = answer.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            let bridge_id = entry.get("id")?.as_str()?.to_owned();
            let host = entry.get("internalipaddress")?.as_str()?.to_owned();
            let port = entry
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(Endpoint::HTTPS_PORT);
            let short: String = bridge_id
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Some(Candidate {
                name: format!("Philips Hue - {}", short.to_ascii_uppercase()),
                bridge_id,
                host,
                https_port: port,
            })
        })
        .collect()
}

/// Finds bridges: mDNS, then (only if that found none) the discovery service. A source that
/// fails is logged and counts as having found nothing.
pub async fn discover(config: &DiscoveryConfig) -> Vec<Candidate> {
    let mut found = match mdns(config).await {
        Ok(found) => found,
        Err(e) => {
            log::warn!("{e}");
            Vec::new()
        }
    };
    if let (true, Some(url)) = (found.is_empty(), &config.cloud_url) {
        match cloud(config, url).await {
            Ok(list) => found = list,
            Err(e) => log::warn!("asking the Hue discovery service failed: {e}"),
        }
    }
    let mut seen = HashSet::new();
    found.retain(|c| seen.insert(c.bridge_id.to_ascii_lowercase()));
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_discovery_services_answer_becomes_candidates() {
        let answer = json!([
            {"id": "001788fffe23bfc2", "internalipaddress": "192.168.1.9", "port": 443},
            {"id": "001788fffe000001", "internalipaddress": "192.168.1.10"},
            {"internalipaddress": "no id"},
        ]);
        let found = candidates_from_cloud(&answer);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Philips Hue - 23BFC2");
        assert_eq!(found[1].https_port, 443);
        assert_eq!(candidates_from_cloud(&json!({"error": "x"})), Vec::new());
    }

    #[test]
    fn a_candidate_lists_with_its_port_only_when_it_is_not_the_default() {
        let mut candidate = candidates_from_cloud(&json!([
            {"id": "001788fffe23bfc2", "internalipaddress": "10.0.0.2", "port": 8443}
        ]))
        .remove(0);
        assert_eq!(&**candidate.device().unwrap().address, "10.0.0.2:8443");
        candidate.https_port = 443;
        assert_eq!(&**candidate.device().unwrap().address, "10.0.0.2");
    }
}
