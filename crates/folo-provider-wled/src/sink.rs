//! Where datagrams go, so tests can record them instead (a paused clock and real sockets don't
//! mix).

use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use folo_provider::BoxFuture;
use tokio::net::{UdpSocket, lookup_host};
use tokio::sync::Mutex;

/// Sends UDP datagrams to the device.
pub trait DatagramSink: Send + Sync {
    /// One datagram to this port of the device.
    fn send(&self, port: u16, payload: Vec<u8>) -> BoxFuture<'_, io::Result<()>>;

    /// Forgets what was resolved of the device's address: its IP may have changed while the
    /// connection was down.
    fn reset(&self) -> BoxFuture<'_, ()>;
}

/// How long a resolved address is used: a device that moves (DHCP) is found again by then.
const ADDRESS_TTL: Duration = Duration::from_secs(60);

/// How long a lookup may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

struct Open {
    socket: UdpSocket,
    ip: std::net::IpAddr,
    resolved: Instant,
}

/// UDP from a socket of our own to the device's host.
pub struct UdpSink {
    host: String,
    socket: Mutex<Option<Open>>,
}

impl UdpSink {
    #[must_use]
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_owned(),
            socket: Mutex::new(None),
        }
    }

    async fn open(&self) -> io::Result<Open> {
        let ip = tokio::time::timeout(LOOKUP_TIMEOUT, lookup_host((self.host.as_str(), 0)))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the host lookup took too long"))??
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the host has no address"))?
            .ip();
        let local: SocketAddr = if ip.is_ipv4() {
            ([0, 0, 0, 0], 0).into()
        } else {
            ([0u16; 8], 0).into()
        };
        Ok(Open {
            socket: UdpSocket::bind(local).await?,
            ip,
            resolved: Instant::now(),
        })
    }
}

impl DatagramSink for UdpSink {
    fn send(&self, port: u16, payload: Vec<u8>) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            let mut guard = self.socket.lock().await;
            if guard
                .as_ref()
                .is_none_or(|o| o.resolved.elapsed() > ADDRESS_TTL)
            {
                *guard = Some(self.open().await?);
            }
            let Some(open) = guard.as_ref() else {
                return Ok(());
            };
            open.socket
                .send_to(&payload, (open.ip, port))
                .await
                .map(|_| ())
        })
    }

    fn reset(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            *self.socket.lock().await = None;
        })
    }
}
