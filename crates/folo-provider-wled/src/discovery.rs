//! Finding WLED devices: mDNS `_wled._tcp`, and manual entry by probing a host.
//!
//! The network part ([`MdnsBrowser`]) sits behind [`Browse`]; what an answer means
//! ([`device_from_answer`]) is plain code.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use folo_protocol::generated::DiscoveredDevice;
use folo_provider::{BoxFuture, ProviderError};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde_json::json;

use crate::api::DeviceApi;
use crate::reader::INFO_PATH;

/// The service WLED announces itself as.
const SERVICE_TYPE: &str = "_wled._tcp.local.";

/// How long one discovery listens for answers.
const BROWSE_WINDOW: Duration = Duration::from_secs(3);

const HTTP_PORT: u16 = 80;

/// A resolved service, as much of it as discovery needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceAnswer {
    /// The instance's name, as WLED announces it (`wled-aabbcc`).
    pub instance: String,
    pub port: u16,
    pub addresses: Vec<IpAddr>,
    /// The TXT record's `key=value` pairs; WLED sends its `mac`.
    pub properties: Vec<(String, String)>,
}

impl ServiceAnswer {
    fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// The instance name of a service's full name (`wled-aabbcc._wled._tcp.local.`).
#[must_use]
pub fn instance_name(fullname: &str) -> &str {
    fullname
        .split_once("._wled.")
        .map_or(fullname, |(instance, _)| instance)
}

/// An address a person can type: the first IPv4 address, else the first IPv6 one, with the port
/// when it isn't 80.
fn address_of(answer: &ServiceAnswer) -> Option<String> {
    let ip = answer
        .addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| answer.addresses.first())?;
    let host = match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    };
    Some(if answer.port == HTTP_PORT || answer.port == 0 {
        host
    } else {
        format!("{host}:{}", answer.port)
    })
}

/// The device an answer announces: its id is the MAC address when the TXT record has one (it
/// stays when the device's name changes), else the instance name. `None` for an answer without
/// an address or with names the contract wouldn't take.
#[must_use]
pub fn device_from_answer(answer: &ServiceAnswer) -> Option<DiscoveredDevice> {
    let id = answer
        .property("mac")
        .map_or_else(|| answer.instance.clone(), str::to_ascii_lowercase);
    serde_json::from_value(json!({
        "kind": "wled",
        "id": id,
        "name": answer.instance,
        "address": address_of(answer)?,
    }))
    .ok()
}

/// Manual entry: asks the host what it is.
///
/// # Errors
/// [`ProviderError`] when the host doesn't answer as a WLED device.
pub async fn probe(api: &dyn DeviceApi) -> Result<DiscoveredDevice, ProviderError> {
    let info = api.get(INFO_PATH).await?;
    let text = |key: &str| {
        info.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    };
    if !text("brand").eq_ignore_ascii_case("WLED") {
        return Err(ProviderError::Failed(format!(
            "{} does not answer as a WLED device",
            api.authority()
        )));
    }
    let id = if text("mac").is_empty() {
        api.authority()
    } else {
        text("mac").to_ascii_lowercase()
    };
    let name = if text("name").is_empty() {
        "WLED"
    } else {
        text("name")
    };
    serde_json::from_value(json!({
        "kind": "wled",
        "id": id,
        "name": name,
        "address": api.authority().trim_end_matches(":80"),
    }))
    .map_err(|e| ProviderError::Failed(format!("the device's answer doesn't fit: {e}")))
}

/// Listens for WLED's announcements.
pub trait Browse: Send + Sync {
    fn browse(&self) -> BoxFuture<'_, Result<Vec<ServiceAnswer>, ProviderError>>;
}

/// mDNS over the machine's network interfaces.
pub struct MdnsBrowser {
    window: Duration,
}

impl Default for MdnsBrowser {
    fn default() -> Self {
        Self {
            window: BROWSE_WINDOW,
        }
    }
}

impl MdnsBrowser {
    fn listen(window: Duration) -> Result<Vec<ServiceAnswer>, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(SERVICE_TYPE)?;
        let deadline = Instant::now() + window;
        let mut found: Vec<ServiceAnswer> = Vec::new();
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = events.recv_timeout(left) else {
                break;
            };
            if let ServiceEvent::ServiceResolved(service) = event {
                let answer = ServiceAnswer {
                    instance: instance_name(&service.fullname).to_owned(),
                    port: service.port,
                    addresses: service
                        .addresses
                        .iter()
                        .map(mdns_sd::ScopedIp::to_ip_addr)
                        .collect(),
                    properties: service
                        .txt_properties
                        .iter()
                        .map(|p| (p.key().to_owned(), p.val_str().to_owned()))
                        .collect(),
                };
                if !found.iter().any(|f| f.instance == answer.instance) {
                    found.push(answer);
                }
            }
        }
        let _ = daemon.stop_browse(SERVICE_TYPE);
        let _ = daemon.shutdown();
        Ok(found)
    }
}

impl Browse for MdnsBrowser {
    fn browse(&self) -> BoxFuture<'_, Result<Vec<ServiceAnswer>, ProviderError>> {
        let window = self.window;
        Box::pin(async move {
            tokio::task::spawn_blocking(move || Self::listen(window))
                .await
                .map_err(|e| ProviderError::Failed(format!("mDNS discovery stopped: {e}")))?
                .map_err(|e| ProviderError::Failed(format!("mDNS discovery failed: {e}")))
        })
    }
}
