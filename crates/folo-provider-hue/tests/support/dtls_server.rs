//! A DTLS-PSK endpoint standing in for the bridge's port 2100: it completes handshakes with the
//! expected identity and key and keeps the decrypted datagrams it is sent.

use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use openssl::ssl::{ErrorCode, Ssl, SslContext, SslMethod, SslOptions, SslVerifyMode, SslVersion};

#[derive(Default)]
pub struct Observed {
    /// The decrypted datagrams, in order.
    pub datagrams: Vec<Vec<u8>>,
    /// The identities clients offered.
    pub identities: Vec<String>,
    /// Sessions that ended with `close_notify`.
    pub closes: u32,
    /// Handshakes that completed.
    pub handshakes: u32,
}

pub struct DtlsServer {
    pub port: u16,
    pub observed: Arc<Mutex<Observed>>,
    /// Datagrams to discard before answering, to make the client retransmit.
    pub drop_first: Arc<AtomicU32>,
    /// Close the session after this many datagrams (0: never).
    pub close_after: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Peer {
    socket: UdpSocket,
    address: SocketAddr,
    drop_first: Arc<AtomicU32>,
}

impl Read for Peer {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let (n, from) = self.socket.recv_from(buf)?;
            if from != self.address {
                continue;
            }
            let dropping = self
                .drop_first
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
            if !dropping {
                return Ok(n);
            }
        }
    }
}

impl Write for Peer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.socket.send_to(buf, self.address)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn context(identity: String, key: Vec<u8>, observed: Arc<Mutex<Observed>>) -> SslContext {
    let mut builder = SslContext::builder(SslMethod::dtls()).unwrap();
    builder
        .set_min_proto_version(Some(SslVersion::DTLS1_2))
        .unwrap();
    builder
        .set_max_proto_version(Some(SslVersion::DTLS1_2))
        .unwrap();
    builder.set_cipher_list("PSK-AES128-GCM-SHA256").unwrap();
    builder.set_verify(SslVerifyMode::NONE);
    builder.set_options(SslOptions::NO_QUERY_MTU);
    builder.set_psk_server_callback(move |_ssl, offered, psk_out| {
        let offered = String::from_utf8_lossy(offered.unwrap_or_default()).into_owned();
        observed.lock().unwrap().identities.push(offered.clone());
        if offered != identity || key.len() > psk_out.len() {
            return Ok(0);
        }
        psk_out[..key.len()].copy_from_slice(&key);
        Ok(key.len())
    });
    builder.build()
}

impl DtlsServer {
    pub fn start(identity: &str, key: &[u8]) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let observed = Arc::new(Mutex::new(Observed::default()));
        let drop_first = Arc::new(AtomicU32::new(0));
        let close_after = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let context = context(identity.to_owned(), key.to_vec(), Arc::clone(&observed));
        let thread = {
            let (observed, drop_first, close_after, stop) = (
                Arc::clone(&observed),
                Arc::clone(&drop_first),
                Arc::clone(&close_after),
                Arc::clone(&stop),
            );
            std::thread::spawn(move || {
                serve(
                    &socket,
                    &context,
                    &observed,
                    &drop_first,
                    &close_after,
                    &stop,
                );
            })
        };
        Self {
            port,
            observed,
            drop_first,
            close_after,
            stop,
            thread: Some(thread),
        }
    }

    /// Waits until `condition` holds of what was observed.
    pub fn wait_for(&self, what: &str, condition: impl Fn(&Observed) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition(&self.observed.lock().unwrap()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the DTLS endpoint never saw {what}");
    }

    /// [`wait_for`](Self::wait_for) for async tests: the runtime keeps running meanwhile.
    pub async fn eventually(&self, what: &str, condition: impl Fn(&Observed) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition(&self.observed.lock().unwrap()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the DTLS endpoint never saw {what}");
    }

    pub fn datagrams(&self) -> Vec<Vec<u8>> {
        self.observed.lock().unwrap().datagrams.clone()
    }
}

impl Drop for DtlsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    socket: &UdpSocket,
    context: &SslContext,
    observed: &Arc<Mutex<Observed>>,
    drop_first: &Arc<AtomicU32>,
    close_after: &Arc<AtomicU32>,
    stop: &Arc<AtomicBool>,
) {
    let mut scratch = [0u8; 2048];
    while !stop.load(Ordering::SeqCst) {
        let Ok((_, peer)) = socket.peek_from(&mut scratch) else {
            continue;
        };
        let stream = Peer {
            socket: socket.try_clone().unwrap(),
            address: peer,
            drop_first: Arc::clone(drop_first),
        };
        let mut ssl = Ssl::new(context).unwrap();
        ssl.set_mtu(1200).unwrap();
        let Ok(mut session) = ssl.accept(stream) else {
            // The handshake didn't complete (a dropped datagram, a wrong key): the datagrams
            // of it are thrown away and the next hello starts again
            let mut sink = [0u8; 2048];
            while socket.recv_from(&mut sink).is_ok() {}
            continue;
        };
        observed.lock().unwrap().handshakes += 1;
        let mut received = 0;
        while !stop.load(Ordering::SeqCst) {
            let mut buf = [0u8; 2048];
            match session.ssl_read(&mut buf) {
                Ok(n) => {
                    observed.lock().unwrap().datagrams.push(buf[..n].to_vec());
                    received += 1;
                    let limit = close_after.load(Ordering::SeqCst);
                    if limit > 0 && received >= limit {
                        let _ = session.shutdown();
                        break;
                    }
                }
                Err(e) if matches!(e.code(), ErrorCode::WANT_READ | ErrorCode::WANT_WRITE) => {}
                Err(e) if e.code() == ErrorCode::ZERO_RETURN => {
                    observed.lock().unwrap().closes += 1;
                    break;
                }
                Err(e)
                    if e.io_error().is_some_and(|io| {
                        matches!(
                            io.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        )
                    }) => {}
                Err(_) => break,
            }
        }
    }
}
