//! One TCP connection to Hyperion's JSON API: newline-terminated JSON out, and a reader task that
//! reads every reply, because Hyperion answers every command and an unread socket fills up.
//! Replies to requests that carry a `tan` go to whoever asked; all others are dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use folo_provider::ProviderError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// As long as the legacy sender waits for a connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a request waits for its reply. The sender limits `connect` and `describe` as a
/// whole; administration has no sender above it.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest reply line read. `serverinfo` of a big setup is some hundred kilobytes.
const MAX_REPLY_BYTES: u64 = 16 * 1024 * 1024;

type Waiters = HashMap<u64, oneshot::Sender<Value>>;

fn lock(waiters: &Mutex<Waiters>) -> MutexGuard<'_, Waiters> {
    waiters
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct Link {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    waiters: Arc<Mutex<Waiters>>,
    next_tan: AtomicU64,
    /// Set when a write failed or timed out: the stream may hold half a line, so nobody
    /// writes to it again.
    broken: AtomicBool,
    reader: JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Link {
    /// Connects to `host:port`.
    ///
    /// # Errors
    /// [`ProviderError::Unreachable`] when Hyperion can't be reached in time.
    pub async fn open(host: &str, port: u16) -> Result<Self, ProviderError> {
        let connecting = TcpStream::connect((host, port));
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                return Err(ProviderError::Unreachable(format!(
                    "can't connect to Hyperion at {host}:{port}: {e}"
                )));
            }
            Err(_) => {
                return Err(ProviderError::Unreachable(format!(
                    "connecting to Hyperion at {host}:{port} timed out"
                )));
            }
        };
        // Frames are small and a late one is worth nothing
        let _ = stream.set_nodelay(true);
        let (read, write) = stream.into_split();
        let waiters = Arc::new(Mutex::new(Waiters::new()));
        let reader = tokio::spawn(read_replies(read, Arc::clone(&waiters)));
        Ok(Self {
            writer: tokio::sync::Mutex::new(write),
            waiters,
            next_tan: AtomicU64::new(1),
            broken: AtomicBool::new(false),
            reader,
        })
    }

    /// Whether the connection can still be used: Hyperion hasn't closed it and no write on it
    /// failed.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.reader.is_finished() && !self.broken.load(Ordering::Relaxed)
    }

    /// Writes one line, which carries its own newline.
    ///
    /// # Errors
    /// [`ProviderError::Unreachable`] when the connection is gone.
    pub async fn send_line(&self, line: &[u8]) -> Result<(), ProviderError> {
        if !self.is_open() {
            return Err(ProviderError::Unreachable(
                "Hyperion closed the connection".to_owned(),
            ));
        }
        let mut writer = self.writer.lock().await;
        // Cancelled mid-line (by the sender's limit, say), the stream holds a part of a line
        let guard = BrokenOnDrop(&self.broken);
        let written = tokio::time::timeout(REQUEST_TIMEOUT, writer.write_all(line)).await;
        match written {
            Ok(Ok(())) => {
                guard.disarm();
                Ok(())
            }
            Ok(Err(e)) => Err(ProviderError::Unreachable(format!(
                "writing to Hyperion failed: {e}"
            ))),
            Err(_) => Err(ProviderError::Unreachable(
                "writing to Hyperion timed out".to_owned(),
            )),
        }
    }

    /// Sends a command with a `tan` and waits for the reply that carries it.
    ///
    /// # Errors
    /// [`ProviderError::Unreachable`] when the connection is lost or Hyperion doesn't answer in
    /// time; [`ProviderError::Refused`] when it answers that the command failed.
    pub async fn request(&self, mut command: Value) -> Result<Value, ProviderError> {
        let tan = self.next_tan.fetch_add(1, Ordering::Relaxed);
        command["tan"] = json!(tan);
        let (answer, reply) = oneshot::channel();
        lock(&self.waiters).insert(tan, answer);
        let mut line = command.to_string().into_bytes();
        line.push(b'\n');
        let outcome = self.exchange(&line, reply).await;
        lock(&self.waiters).remove(&tan);
        let reply = outcome?;
        if reply["success"].as_bool() == Some(true) {
            return Ok(reply);
        }
        let name = command["command"].as_str().unwrap_or("command");
        let why = reply["error"].as_str().unwrap_or("no reason given");
        Err(ProviderError::Refused(format!(
            "Hyperion refused {name}: {why}"
        )))
    }

    async fn exchange(
        &self,
        line: &[u8],
        reply: oneshot::Receiver<Value>,
    ) -> Result<Value, ProviderError> {
        self.send_line(line).await?;
        match tokio::time::timeout(REQUEST_TIMEOUT, reply).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(ProviderError::Unreachable(
                "Hyperion closed the connection".to_owned(),
            )),
            Err(_) => Err(ProviderError::Unreachable(
                "Hyperion didn't answer in time".to_owned(),
            )),
        }
    }

    /// Closes the connection after what was written has gone out.
    pub async fn close(&self) {
        let mut writer = self.writer.lock().await;
        let _ = writer.shutdown().await;
    }
}

/// Marks a link broken unless disarmed: a write that was cancelled, failed or timed out.
struct BrokenOnDrop<'a>(&'a AtomicBool);

impl BrokenOnDrop<'_> {
    fn disarm(self) {
        std::mem::forget(self);
    }
}

impl Drop for BrokenOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Reads reply lines until the connection ends, passing on those somebody waits for. When it
/// ends, the waiters are dropped, which tells them the connection is gone.
async fn read_replies(read: tokio::net::tcp::OwnedReadHalf, waiters: Arc<Mutex<Waiters>>) {
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        line.clear();
        let Ok(n) = (&mut reader)
            .take(MAX_REPLY_BYTES)
            .read_until(b'\n', &mut line)
            .await
        else {
            break;
        };
        if n == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            log::warn!("a reply from Hyperion has no end within {MAX_REPLY_BYTES} bytes");
            break;
        }
        let Ok(reply) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some(tan) = reply["tan"].as_u64() else {
            continue;
        };
        if let Some(waiter) = lock(&waiters).remove(&tan) {
            let _ = waiter.send(reply);
        }
    }
    lock(&waiters).clear();
}
