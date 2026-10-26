//! The Entertainment API's transport: DTLS 1.2 with a pre-shared key over UDP
//! (`docs/integrations/hue.md`, DTLS; ADR-0013 D2).
//!
//! The cipher is `TLS_PSK_WITH_AES_128_GCM_SHA256`. The PSK identity is the application id
//! (the username when the bridge has none) and the key is the hex-decoded client key.
//!
//! OpenSSL's retransmission timers need a datagram BIO, which the `openssl` crate doesn't offer
//! over a socket, so the retransmits are done here: an attempt that gets no answer within a
//! random 600–1000 ms starts the handshake over on the same socket, up to five attempts.
//!
//! Sending is a write on a non-blocking UDP socket, which takes microseconds, so a session is
//! used from async code without a thread of its own. The handshake and the close block, and
//! are run on a blocking thread by the caller.

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use openssl::error::ErrorStack;
use openssl::ssl::{
    ErrorCode, HandshakeError, Ssl, SslContext, SslMethod, SslOptions, SslStream, SslVerifyMode,
    SslVersion,
};

/// The cipher the bridge takes.
const CIPHER: &str = "PSK-AES128-GCM-SHA256";

/// How a session is made.
#[derive(Clone)]
pub struct DtlsSettings {
    /// The PSK identity.
    pub identity: String,
    /// The PSK.
    pub key: Vec<u8>,
    /// Handshake attempts before giving up.
    pub attempts: u32,
    /// An attempt waits for the bridge's answer between these two limits, chosen at random.
    pub retransmit_min: Duration,
    pub retransmit_max: Duration,
    /// The datagram size the handshake fragments to, below an ethernet MTU.
    pub mtu: u32,
}

impl DtlsSettings {
    /// The run book's values for a bridge: five attempts of 600–1000 ms.
    #[must_use]
    pub fn new(identity: impl Into<String>, key: Vec<u8>) -> Self {
        Self {
            identity: identity.into(),
            key,
            attempts: 5,
            retransmit_min: Duration::from_millis(600),
            retransmit_max: Duration::from_millis(1000),
            mtu: 1200,
        }
    }
}

impl fmt::Debug for DtlsSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DtlsSettings")
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

/// What went wrong with a stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DtlsError {
    /// The handshake didn't complete.
    Handshake(String),
    /// The bridge closed the stream (a `close_notify`, or it stopped answering).
    Closed(String),
    /// Anything else the socket or OpenSSL reports.
    Io(String),
}

impl fmt::Display for DtlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshake(m) => write!(f, "the DTLS handshake failed: {m}"),
            Self::Closed(m) => write!(f, "the stream was closed: {m}"),
            Self::Io(m) => write!(f, "the stream failed: {m}"),
        }
    }
}

impl std::error::Error for DtlsError {}

fn io(e: impl fmt::Display) -> DtlsError {
    DtlsError::Io(e.to_string())
}

/// A random duration between `low` and `high`.
fn between(low: Duration, high: Duration) -> Duration {
    let mut bytes = [0u8; 4];
    let unit = if getrandom::fill(&mut bytes).is_ok() {
        f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX)
    } else {
        0.0
    };
    low + high.saturating_sub(low).mul_f64(unit)
}

fn context(settings: &DtlsSettings) -> Result<SslContext, ErrorStack> {
    let mut builder = SslContext::builder(SslMethod::dtls())?;
    builder.set_min_proto_version(Some(SslVersion::DTLS1_2))?;
    builder.set_max_proto_version(Some(SslVersion::DTLS1_2))?;
    builder.set_cipher_list(CIPHER)?;
    builder.set_verify(SslVerifyMode::NONE);
    // The socket isn't a datagram BIO, so OpenSSL can't ask it for the path's MTU
    builder.set_options(SslOptions::NO_QUERY_MTU);
    let identity = settings.identity.clone();
    let key = settings.key.clone();
    builder.set_psk_client_callback(move |_ssl, _hint, identity_out, key_out| {
        let (identity, key) = (identity.as_bytes(), key.as_slice());
        // The identity buffer takes a NUL-terminated string
        if identity.len() + 1 > identity_out.len() || key.len() > key_out.len() {
            return Ok(0);
        }
        identity_out[..identity.len()].copy_from_slice(identity);
        identity_out[identity.len()] = 0;
        key_out[..key.len()].copy_from_slice(key);
        Ok(key.len())
    });
    Ok(builder.build())
}

/// A connected UDP socket as the byte stream OpenSSL wants: a read is one datagram, a write
/// sends one.
struct Datagrams(UdpSocket);

impl Read for Datagrams {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Write for Datagrams {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A DTLS session with a bridge.
pub struct DtlsSession {
    stream: SslStream<Datagrams>,
}

impl fmt::Debug for DtlsSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DtlsSession")
    }
}

enum Attempt {
    Done(SslStream<Datagrams>),
    /// No answer in time: try again.
    Silent,
    Failed(DtlsError),
}

impl DtlsSession {
    /// Shakes hands with the bridge at `address`. This blocks for as long as the attempts take
    /// (five seconds at most with the default settings).
    ///
    /// # Errors
    /// [`DtlsError::Handshake`] when no attempt completes, or the bridge refuses the identity.
    pub fn connect(address: SocketAddr, settings: &DtlsSettings) -> Result<Self, DtlsError> {
        let context = context(settings).map_err(io)?;
        let local: SocketAddr = if address.is_ipv4() {
            ([0, 0, 0, 0], 0).into()
        } else {
            ([0u16; 8], 0).into()
        };
        let socket = UdpSocket::bind(local).map_err(io)?;
        socket.connect(address).map_err(io)?;
        for attempt in 1..=settings.attempts.max(1) {
            drain(&socket);
            let wait = between(settings.retransmit_min, settings.retransmit_max);
            match Self::attempt(&socket, &context, settings.mtu, wait)? {
                Attempt::Done(stream) => {
                    stream.get_ref().0.set_nonblocking(true).map_err(io)?;
                    return Ok(Self { stream });
                }
                Attempt::Silent => log::debug!("DTLS handshake attempt {attempt} got no answer"),
                Attempt::Failed(e) => return Err(e),
            }
        }
        Err(DtlsError::Handshake(format!(
            "no answer from the bridge after {} attempts",
            settings.attempts.max(1)
        )))
    }

    fn attempt(
        socket: &UdpSocket,
        context: &SslContext,
        mtu: u32,
        wait: Duration,
    ) -> Result<Attempt, DtlsError> {
        socket.set_nonblocking(false).map_err(io)?;
        socket.set_read_timeout(Some(wait)).map_err(io)?;
        let mut ssl = Ssl::new(context).map_err(io)?;
        ssl.set_mtu(mtu).map_err(io)?;
        let stream = Datagrams(socket.try_clone().map_err(io)?);
        Ok(match ssl.connect(stream) {
            Ok(stream) => Attempt::Done(stream),
            Err(HandshakeError::WouldBlock(_)) => Attempt::Silent,
            Err(HandshakeError::Failure(mid)) => {
                Attempt::Failed(DtlsError::Handshake(mid.error().to_string()))
            }
            Err(HandshakeError::SetupFailure(e)) => {
                Attempt::Failed(DtlsError::Handshake(e.to_string()))
            }
        })
    }

    /// Sends one datagram, then looks (without waiting) for the bridge having closed the stream.
    /// A datagram the socket has no room for is dropped: the next frame replaces it.
    ///
    /// # Errors
    /// [`DtlsError::Closed`] when the bridge closed the stream, [`DtlsError::Io`] for the rest.
    pub fn send(&mut self, packet: &[u8]) -> Result<(), DtlsError> {
        match self.stream.ssl_write(packet) {
            Ok(_) => {}
            Err(e) if matches!(e.code(), ErrorCode::WANT_WRITE | ErrorCode::WANT_READ) => {}
            Err(e) => return Err(io(e)),
        }
        self.look_for_close()
    }

    fn look_for_close(&mut self) -> Result<(), DtlsError> {
        let mut scratch = [0u8; 64];
        match self.stream.ssl_read(&mut scratch) {
            // The bridge doesn't send application data
            Ok(_) => Ok(()),
            Err(e) => match e.code() {
                ErrorCode::WANT_READ | ErrorCode::WANT_WRITE => Ok(()),
                ErrorCode::ZERO_RETURN => {
                    Err(DtlsError::Closed("close_notify from the bridge".into()))
                }
                _ => Err(DtlsError::Closed(e.to_string())),
            },
        }
    }

    /// Sends `close_notify` and drops the socket. It never fails: a bridge that doesn't get the
    /// alert closes the stream after ten seconds of silence.
    pub fn close(mut self) {
        if let Err(e) = self.stream.shutdown() {
            log::debug!("sending close_notify failed: {e}");
        }
    }
}

/// Throws away datagrams that arrived after their attempt was given up.
fn drain(socket: &UdpSocket) {
    if socket.set_nonblocking(true).is_err() {
        return;
    }
    let mut scratch = [0u8; 2048];
    while socket.recv(&mut scratch).is_ok() {}
}
