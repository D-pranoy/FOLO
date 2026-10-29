//! Where a bridge is, on the network.

use std::net::SocketAddr;

use folo_provider::ProviderError;

/// The address of a bridge and the ports FOLO talks to. The ports are the bridge's own unless a
/// test double listens elsewhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    /// An IP address or a host name, without a port.
    pub host: String,
    /// The HTTPS port (REST).
    pub https_port: u16,
    /// The UDP port of the Entertainment API (DTLS).
    pub dtls_port: u16,
}

impl Endpoint {
    /// The HTTPS port of a bridge.
    pub const HTTPS_PORT: u16 = 443;
    /// The Entertainment API's UDP port.
    pub const DTLS_PORT: u16 = 2100;

    /// A bridge at `host` on its own ports.
    #[must_use]
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            https_port: Self::HTTPS_PORT,
            dtls_port: Self::DTLS_PORT,
        }
    }

    /// An address as a person or a discovery gives it: `host` or `host:https_port`, where an
    /// IPv6 address is in brackets when it has a port.
    ///
    /// # Errors
    /// When the address is empty or its port isn't a number.
    pub fn parse(address: &str) -> Result<Self, ProviderError> {
        let address = address.trim();
        let address = address
            .strip_prefix("https://")
            .unwrap_or(address)
            .trim_end_matches('/');
        if address.is_empty() {
            return Err(ProviderError::Failed("the bridge has no address".into()));
        }
        if address.contains(['/', '@', '%', ' ', '?', '#']) {
            return Err(ProviderError::Failed(format!(
                "bad bridge address {address:?}"
            )));
        }
        if let Some(rest) = address.strip_prefix('[') {
            let (host, tail) = rest
                .split_once(']')
                .ok_or_else(|| ProviderError::Failed(format!("bad bridge address {address}")))?;
            return Self::with_port(host, tail.strip_prefix(':'));
        }
        // More than one colon without brackets is a bare IPv6 address
        if address.matches(':').count() > 1 {
            return Ok(Self::new(address));
        }
        match address.split_once(':') {
            Some((host, port)) => Self::with_port(host, Some(port)),
            None => Ok(Self::new(address)),
        }
    }

    fn with_port(host: &str, port: Option<&str>) -> Result<Self, ProviderError> {
        if host.is_empty() || host.contains(['/', '@', '%', ' ', '?', '#']) {
            return Err(ProviderError::Failed(format!(
                "bad bridge address {host:?}"
            )));
        }
        let mut endpoint = Self::new(host);
        if let Some(port) = port.filter(|p| !p.is_empty()) {
            endpoint.https_port = port
                .parse()
                .map_err(|_| ProviderError::Failed(format!("bad port {port} in the address")))?;
        }
        Ok(endpoint)
    }

    /// `https://host:port`, with an IPv6 host in brackets.
    #[must_use]
    pub fn base_url(&self) -> String {
        if self.host.contains(':') {
            format!("https://[{}]:{}", self.host, self.https_port)
        } else {
            format!("https://{}:{}", self.host, self.https_port)
        }
    }

    /// The DTLS socket address, resolving a host name when there is one.
    ///
    /// # Errors
    /// [`ProviderError::Unreachable`] when the host doesn't resolve.
    pub async fn dtls_address(&self) -> Result<SocketAddr, ProviderError> {
        let target = (self.host.as_str(), self.dtls_port);
        tokio::net::lookup_host(target)
            .await
            .map_err(|e| ProviderError::Unreachable(format!("resolving {}: {e}", self.host)))?
            .next()
            .ok_or_else(|| ProviderError::Unreachable(format!("{} has no address", self.host)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_ports_and_ipv6() {
        assert_eq!(
            Endpoint::parse("192.168.1.9").unwrap(),
            Endpoint::new("192.168.1.9")
        );
        let with_port = Endpoint::parse("https://bridge.local:8443/").unwrap();
        assert_eq!(
            (with_port.host.as_str(), with_port.https_port),
            ("bridge.local", 8443)
        );
        let v6 = Endpoint::parse("[fe80::1]:444").unwrap();
        assert_eq!((v6.host.as_str(), v6.https_port), ("fe80::1", 444));
        assert_eq!(v6.base_url(), "https://[fe80::1]:444");
        assert_eq!(Endpoint::parse("fe80::1").unwrap().host, "fe80::1");
        assert!(Endpoint::parse("  ").is_err());
        assert!(Endpoint::parse("host:abc").is_err());
        assert!(Endpoint::parse("host/path").is_err());
        assert!(Endpoint::parse("user@host").is_err());
        assert!(Endpoint::parse("fe80::1%eth0").is_err());
    }
}
