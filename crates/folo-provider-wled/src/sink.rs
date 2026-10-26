//! Where datagrams go, so tests can record them instead (a paused clock and real sockets don't
//! mix).

use std::io;
use std::net::SocketAddr;

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

/// UDP from a socket of our own to the device's host.
pub struct UdpSink {
    host: String,
    socket: Mutex<Option<(UdpSocket, std::net::IpAddr)>>,
}

impl UdpSink {
    #[must_use]
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_owned(),
            socket: Mutex::new(None),
        }
    }

    async fn open(&self) -> io::Result<(UdpSocket, std::net::IpAddr)> {
        let ip = lookup_host((self.host.as_str(), 0))
            .await?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the host has no address"))?
            .ip();
        let local: SocketAddr = if ip.is_ipv4() {
            ([0, 0, 0, 0], 0).into()
        } else {
            ([0u16; 8], 0).into()
        };
        Ok((UdpSocket::bind(local).await?, ip))
    }
}

impl DatagramSink for UdpSink {
    fn send(&self, port: u16, payload: Vec<u8>) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            let mut guard = self.socket.lock().await;
            if guard.is_none() {
                *guard = Some(self.open().await?);
            }
            let Some((socket, ip)) = guard.as_ref() else {
                return Ok(());
            };
            socket.send_to(&payload, (*ip, port)).await.map(|_| ())
        })
    }

    fn reset(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            *self.socket.lock().await = None;
        })
    }
}
