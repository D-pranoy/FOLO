//! An instance's settings, as the provider uses them.

use folo_protocol::generated::{ProviderInstance, ProviderSecrets, ProviderSettings};
use folo_provider::ProviderError;

use crate::ddp::DDP_PORT;

/// Which realtime protocol carries the frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    Ddp,
    Dnrgb,
}

/// What a provider needs of its instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// `host` or `host:port` for the HTTP API.
    pub authority: String,
    /// The host alone, for UDP.
    pub host: String,
    pub transport: Transport,
    /// WLED's realtime timeout in seconds, which DNRGB also puts in every packet.
    pub timeout_s: u64,
    /// The device's settings PIN, when it has one.
    pub pin: Option<String>,
    pub ddp_port: u16,
    /// Overrides the device's own UDP port (`info.udpport`).
    pub dnrgb_port: Option<u16>,
}

/// `host`, `host:port`, `[v6]` or `[v6]:port`, as the host alone.
fn bare_host(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            host
        }
        _ => authority,
    }
}

impl Config {
    /// The configuration of an instance.
    ///
    /// # Errors
    /// [`ProviderError::Failed`] when the instance isn't a WLED one or has no host.
    pub fn from_instance(
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Self, ProviderError> {
        let ProviderSettings::WledSettings(settings) = &instance.settings else {
            return Err(ProviderError::Failed(
                "the instance's settings are not WLED's".into(),
            ));
        };
        let authority = settings.host.trim().to_owned();
        if authority.is_empty() {
            return Err(ProviderError::Failed("the instance has no host".into()));
        }
        let transport = match settings.transport {
            folo_protocol::generated::WledSettingsTransport::Ddp => Transport::Ddp,
            folo_protocol::generated::WledSettingsTransport::Dnrgb => Transport::Dnrgb,
        };
        Ok(Self {
            host: bare_host(&authority).to_owned(),
            authority,
            transport,
            timeout_s: settings.timeout_s.get(),
            pin: secrets
                .and_then(|s| s.pin.as_ref())
                .map(|p| (**p).clone())
                .filter(|p| !p.is_empty()),
            ddp_port: DDP_PORT,
            dnrgb_port: None,
        })
    }
}
