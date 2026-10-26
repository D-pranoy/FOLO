//! A fake WLED device for the tests of this crate and the node's: its JSON API (in memory, or
//! served over HTTP on loopback), a UDP receiver that parses DDP and DNRGB(W) the way WLED does,
//! and fixtures of devices.
//!
//! Nothing here touches the network beyond loopback.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use folo_provider::{BoxFuture, ProviderError};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::api::{DeviceApi, status_error};
use crate::sink::DatagramSink;

/// The names of the fixtures, each a device as its three documents say it.
pub const FIXTURES: &[&str] = &[
    "rgb-strip",
    "segments",
    "rgbw-strip",
    "rgbw-auto-white",
    "cct-strip",
    "analog-and-relay",
    "analog-rgb",
    "on-off",
    "matrix-2d",
    "main-segment-only",
];

/// A fixture's `{"info", "state", "cfg"}`.
///
/// # Panics
/// For a name that isn't in [`FIXTURES`].
#[must_use]
pub fn fixture(name: &str) -> Value {
    let text = match name {
        "rgb-strip" => include_str!("../fixtures/rgb-strip.json"),
        "segments" => include_str!("../fixtures/segments.json"),
        "rgbw-strip" => include_str!("../fixtures/rgbw-strip.json"),
        "rgbw-auto-white" => include_str!("../fixtures/rgbw-auto-white.json"),
        "cct-strip" => include_str!("../fixtures/cct-strip.json"),
        "analog-and-relay" => include_str!("../fixtures/analog-and-relay.json"),
        "analog-rgb" => include_str!("../fixtures/analog-rgb.json"),
        "on-off" => include_str!("../fixtures/on-off.json"),
        "matrix-2d" => include_str!("../fixtures/matrix-2d.json"),
        "main-segment-only" => include_str!("../fixtures/main-segment-only.json"),
        other => panic!("no fixture called {other}"),
    };
    serde_json::from_str(text).expect("fixtures are JSON")
}

/// A request the fake device saw.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}

struct Inner {
    info: Value,
    state: Value,
    cfg: Value,
    pin: Option<String>,
    unlocked: bool,
    rebooted: bool,
    requests: Vec<Recorded>,
}

/// A device's JSON API in memory: it answers `/json/info`, `/json/state` and `/json/cfg`, and
/// applies the writes the provider makes (power, brightness, segments, `id.name`, `if.live`,
/// `rb`), so a second read shows them. With a PIN, `/json/cfg` answers 401 until a `POST`
/// carries it as `pin`, as WLED does.
pub struct FakeDevice {
    inner: Mutex<Inner>,
}

fn merge(into: &mut Value, from: &Value) {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in b {
                merge(a.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (slot, value) => *slot = value.clone(),
    }
}

impl FakeDevice {
    /// From `{"info", "state", "cfg"}`.
    #[must_use]
    pub fn from_json(documents: &Value) -> Self {
        Self {
            inner: Mutex::new(Inner {
                info: documents["info"].clone(),
                state: documents["state"].clone(),
                cfg: documents["cfg"].clone(),
                pin: None,
                unlocked: false,
                rebooted: false,
                requests: Vec::new(),
            }),
        }
    }

    /// A fixture of [`FIXTURES`].
    #[must_use]
    pub fn fixture(name: &str) -> Self {
        Self::from_json(&fixture(name))
    }

    /// With a settings PIN.
    #[must_use]
    pub fn with_pin(self, pin: &str) -> Self {
        self.lock().pin = Some(pin.to_owned());
        self
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Changes the documents, for a test that needs a variation of a fixture.
    pub fn edit(&self, change: impl FnOnce(&mut Value, &mut Value, &mut Value)) {
        let mut inner = self.lock();
        let Inner {
            info, state, cfg, ..
        } = &mut *inner;
        change(info, state, cfg);
    }

    #[must_use]
    pub fn info(&self) -> Value {
        self.lock().info.clone()
    }

    #[must_use]
    pub fn state(&self) -> Value {
        self.lock().state.clone()
    }

    #[must_use]
    pub fn cfg(&self) -> Value {
        self.lock().cfg.clone()
    }

    /// Whether a `rb` was received.
    #[must_use]
    pub fn rebooted(&self) -> bool {
        self.lock().rebooted
    }

    /// Every request so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<Recorded> {
        self.lock().requests.clone()
    }

    /// The bodies of the `POST`s so far to this path.
    #[must_use]
    pub fn posts(&self, path: &str) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST" && r.path == path)
            .filter_map(|r| r.body)
            .collect()
    }

    /// Answers a request: the status and the JSON body.
    pub fn handle(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let path = path.split('?').next().unwrap_or(path);
        let mut inner = self.lock();
        inner.requests.push(Recorded {
            method: method.to_owned(),
            path: path.to_owned(),
            body: body.clone(),
        });
        match method {
            "GET" => Self::get(&inner, path),
            "POST" => Self::post(&mut inner, path, &body.unwrap_or(Value::Null)),
            _ => (405, json!({"error": 405})),
        }
    }

    fn locked(inner: &Inner) -> bool {
        inner.pin.is_some() && !inner.unlocked
    }

    fn get(inner: &Inner, path: &str) -> (u16, Value) {
        match path {
            "/json/info" => (200, inner.info.clone()),
            "/json/state" => (200, inner.state.clone()),
            "/json/cfg" if Self::locked(inner) => (401, json!({"error": 1})),
            "/json/cfg" => (200, inner.cfg.clone()),
            _ => (404, json!({"error": 404})),
        }
    }

    fn post(inner: &mut Inner, path: &str, body: &Value) -> (u16, Value) {
        if let (Some(sent), Some(pin)) = (body.get("pin").and_then(Value::as_str), &inner.pin)
            && sent == pin
        {
            inner.unlocked = true;
        }
        match path {
            "/json/state" => Self::post_state(inner, body),
            "/json/cfg" if Self::locked(inner) => return (401, json!({"error": 1})),
            "/json/cfg" => Self::post_cfg(inner, body),
            _ => return (404, json!({"error": 404})),
        }
        (200, json!({"success": true}))
    }

    fn post_state(inner: &mut Inner, body: &Value) {
        let Some(map) = body.as_object() else { return };
        for (key, value) in map {
            match key.as_str() {
                "on" | "bri" => inner.state[key] = value.clone(),
                "rb" => inner.rebooted = value.as_bool().unwrap_or(false),
                "seg" => Self::post_segments(&mut inner.state, value),
                _ => {}
            }
        }
    }

    fn post_segments(state: &mut Value, entries: &Value) {
        let mut segments: Vec<Value> = state["seg"].as_array().cloned().unwrap_or_default();
        for entry in entries.as_array().into_iter().flatten() {
            let Some(id) = entry.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let position = segments.iter().position(|s| s["id"].as_u64() == Some(id));
            if entry.get("stop").and_then(Value::as_u64) == Some(0) {
                if let Some(position) = position {
                    segments.remove(position);
                }
                continue;
            }
            let position = position.unwrap_or_else(|| {
                segments.push(json!({
                    "id": id, "start": 0, "stop": 0, "len": 0, "grp": 1, "spc": 0, "of": 0,
                    "on": true, "bri": 255, "cct": 127, "n": "", "lc": 1
                }));
                segments.len() - 1
            });
            let slot = &mut segments[position];
            merge(slot, entry);
            let (start, stop) = (slot["start"].as_u64(), slot["stop"].as_u64());
            if let (Some(start), Some(stop)) = (start, stop) {
                slot["len"] = json!(stop.saturating_sub(start));
            }
        }
        segments.sort_by_key(|s| s["id"].as_u64());
        state["seg"] = Value::Array(segments);
    }

    fn post_cfg(inner: &mut Inner, body: &Value) {
        let mut settings = body.clone();
        if let Some(map) = settings.as_object_mut() {
            map.remove("pin");
        }
        merge(&mut inner.cfg, &settings);
        if let Some(name) = settings["id"]["name"].as_str() {
            inner.info["name"] = json!(name);
        }
    }
}

impl DeviceApi for FakeDevice {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            let (status, body) = self.handle("GET", path, None);
            status_error(status, path).map_or(Ok(body), Err)
        })
    }

    fn post<'a>(
        &'a self,
        path: &'a str,
        body: &'a Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            let (status, answer) = self.handle("POST", path, Some(body.clone()));
            status_error(status, path).map_or(Ok(answer), Err)
        })
    }

    fn authority(&self) -> String {
        "wled.test".to_owned()
    }
}

/// A [`FakeDevice`] served over HTTP on loopback.
pub struct FakeHttpServer {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for FakeHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = raw[header_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, body))
}

async fn serve(mut stream: tokio::net::TcpStream, device: Arc<FakeDevice>, chunked: bool) {
    let Some((method, path, body)) = read_request(&mut stream).await else {
        return;
    };
    let parsed = serde_json::from_slice::<Value>(&body).ok();
    let (status, answer) = device.handle(&method, &path, parsed);
    let text = answer.to_string();
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
        reason(status)
    );
    let framed = if chunked {
        // Two chunks, to exercise reassembly
        let (a, b) = text.split_at(text.len() / 2);
        format!(
            "{head}Transfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
            a.len(),
            b.len()
        )
    } else {
        format!("{head}Content-Length: {}\r\n\r\n{text}", text.len())
    };
    let _ = stream.write_all(framed.as_bytes()).await;
}

impl FakeHttpServer {
    /// Serves `device` on a free loopback port, with `Content-Length` answers.
    ///
    /// # Panics
    /// When loopback can't be bound.
    pub async fn start(device: Arc<FakeDevice>) -> Self {
        Self::start_with(device, false).await
    }

    /// Like [`start`](Self::start), answering in chunks when `chunked`.
    ///
    /// # Panics
    /// When loopback can't be bound.
    pub async fn start_with(device: Arc<FakeDevice>, chunked: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("bound");
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, device.clone(), chunked));
            }
        });
        Self { addr, task }
    }

    /// `host:port`, as an instance's `host` setting takes it.
    #[must_use]
    pub fn authority(&self) -> String {
        self.addr.to_string()
    }
}

/// A DDP packet as WLED reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DdpPacket {
    pub flags: u8,
    pub sequence: u8,
    pub data_type: u8,
    pub destination: u8,
    pub offset: u32,
    pub data: Vec<u8>,
}

impl DdpPacket {
    /// Reads a packet, refusing what WLED would: a short header, a version other than 1 or data
    /// shorter than the length says.
    ///
    /// # Errors
    /// What is wrong with the packet.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 10 {
            return Err("shorter than the 10-byte header".into());
        }
        if bytes[0] & 0xC0 != 0x40 {
            return Err(format!("not DDP version 1: flags {:#04x}", bytes[0]));
        }
        let length = usize::from(u16::from_be_bytes([bytes[8], bytes[9]]));
        let data = bytes
            .get(10..10 + length)
            .ok_or("the data is shorter than the header says")?;
        if bytes.len() != 10 + length {
            return Err("bytes after the data".into());
        }
        Ok(Self {
            flags: bytes[0],
            sequence: bytes[1],
            data_type: bytes[2],
            destination: bytes[3],
            offset: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            data: data.to_vec(),
        })
    }

    /// The push flag: render the frame now.
    #[must_use]
    pub fn push(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Bytes per pixel, as WLED derives it from the data type: `0x1B` is RGBW, any other RGB.
    #[must_use]
    pub fn channels(&self) -> usize {
        if (self.data_type & 0b0011_1000) >> 3 == 0b011 {
            4
        } else {
            3
        }
    }
}

/// A DNRGB or DNRGBW packet as WLED reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnrgbPacket {
    pub protocol: u8,
    pub timeout_s: u8,
    pub start: u16,
    pub data: Vec<u8>,
}

impl DnrgbPacket {
    /// Reads a packet of protocol 4 or 5. WLED ignores one with fewer than two pixels, and so
    /// does this.
    ///
    /// # Errors
    /// What is wrong with the packet.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let protocol = *bytes.first().ok_or("empty")?;
        if protocol != 4 && protocol != 5 {
            return Err(format!("not DNRGB(W): protocol {protocol}"));
        }
        if bytes.len() < 4 {
            return Err("shorter than the 4-byte header".into());
        }
        let pixel = if protocol == 4 { 3 } else { 4 };
        if bytes.len() > 4 && !(bytes.len() - 4).is_multiple_of(pixel) {
            return Err("a partial pixel".into());
        }
        if bytes.len() > 4 && bytes.len() < 4 + 2 * pixel {
            return Err("WLED ignores a packet of one pixel".into());
        }
        Ok(Self {
            protocol,
            timeout_s: bytes[1],
            start: u16::from_be_bytes([bytes[2], bytes[3]]),
            data: bytes[4..].to_vec(),
        })
    }

    /// A packet with timeout 0 gives the lights back.
    #[must_use]
    pub fn releases(&self) -> bool {
        self.timeout_s == 0
    }
}

/// Collects the packets of frames the way WLED does after it has seen a push: pixels are
/// written into a buffer, and the frame is rendered only when a packet has the push flag.
#[derive(Debug, Default)]
pub struct DdpFrames {
    pixels: Vec<u8>,
    /// The frames rendered so far, as bytes (RGB or RGBW, as the packets were).
    pub rendered: Vec<Vec<u8>>,
}

impl DdpFrames {
    /// Writes a packet; renders when it pushes.
    pub fn apply(&mut self, packet: &DdpPacket) {
        let start = packet.offset as usize;
        let end = start + packet.data.len();
        if self.pixels.len() < end {
            self.pixels.resize(end, 0);
        }
        self.pixels[start..end].copy_from_slice(&packet.data);
        if packet.push() {
            self.rendered.push(self.pixels.clone());
        }
    }
}

/// A UDP socket on loopback that keeps every datagram it gets.
pub struct FakeUdpReceiver {
    addr: SocketAddr,
    datagrams: Arc<Mutex<Vec<Vec<u8>>>>,
    task: JoinHandle<()>,
}

impl Drop for FakeUdpReceiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeUdpReceiver {
    /// Listens on a free loopback port.
    ///
    /// # Panics
    /// When loopback can't be bound.
    pub async fn bind() -> Self {
        let socket = UdpSocket::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback");
        let addr = socket.local_addr().expect("bound");
        let datagrams = Arc::new(Mutex::new(Vec::new()));
        let kept = datagrams.clone();
        let task = tokio::spawn(async move {
            let mut buffer = vec![0u8; 2048];
            while let Ok(n) = socket.recv(&mut buffer).await {
                kept.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(buffer[..n].to_vec());
            }
        });
        Self {
            addr,
            datagrams,
            task,
        }
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Every datagram so far.
    #[must_use]
    pub fn datagrams(&self) -> Vec<Vec<u8>> {
        self.datagrams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Waits (in real time) until at least `count` datagrams have come.
    ///
    /// # Panics
    /// When they haven't come within two seconds.
    pub async fn wait_for(&self, count: usize) -> Vec<Vec<u8>> {
        for _ in 0..200 {
            let all = self.datagrams();
            if all.len() >= count {
                return all;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {count} datagrams, got {}", self.datagrams().len());
    }
}

/// A [`DatagramSink`] that records what it is sent, for tests with a paused clock.
#[derive(Default)]
pub struct MemorySink {
    sent: Mutex<Vec<(Instant, u16, Vec<u8>)>>,
    resets: Mutex<u32>,
}

impl MemorySink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The datagrams so far, with their ports.
    #[must_use]
    pub fn sent(&self) -> Vec<(u16, Vec<u8>)> {
        self.timed()
            .into_iter()
            .map(|(_, port, payload)| (port, payload))
            .collect()
    }

    /// The datagrams so far, with when each was sent on Tokio's clock (which tests pause).
    #[must_use]
    pub fn timed(&self) -> Vec<(Instant, u16, Vec<u8>)> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How often [`reset`](DatagramSink::reset) was called.
    #[must_use]
    pub fn resets(&self) -> u32 {
        *self
            .resets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DatagramSink for MemorySink {
    fn send(&self, port: u16, payload: Vec<u8>) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async move {
            self.sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((Instant::now(), port, payload));
            Ok(())
        })
    }

    fn reset(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            *self
                .resets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        })
    }
}

/// Segments of a state document as `(id, start, stop)`, for assertions.
#[must_use]
pub fn segments_of(state: &Value) -> Vec<(u64, u64, u64)> {
    let empty = Vec::new();
    state["seg"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|s| {
            (
                s["id"].as_u64().unwrap_or(0),
                s["start"].as_u64().unwrap_or(0),
                s["stop"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}
