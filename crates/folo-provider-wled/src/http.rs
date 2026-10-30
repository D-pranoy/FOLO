//! A small HTTP/1.1 client for WLED's JSON API: one request per connection, plain HTTP.
//!
//! WLED's web server answers with a `Content-Length` or in chunks, and closes the connection when
//! asked. Nothing else is needed, so this takes no HTTP crate.

use std::fmt::Write as _;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The most a response may hold. `/json/cfg` of a large setup is a few tens of kilobytes.
const MAX_RESPONSE_BYTES: usize = 1 << 20;

/// How long a whole request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why a request failed before an answer came.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpError {
    /// Connecting, sending or reading failed, or it took too long.
    Io(String),
    /// The answer isn't HTTP/1.x we can read.
    Malformed(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) | Self::Malformed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<std::io::Error> for HttpError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

/// Sends requests to one `host[:port]`.
#[derive(Clone, Debug)]
pub struct HttpClient {
    authority: String,
    timeout: Duration,
}

impl HttpClient {
    /// `authority` is `host` or `host:port`; the port defaults to 80.
    #[must_use]
    pub fn new(authority: &str) -> Self {
        Self {
            authority: with_port(authority),
            timeout: REQUEST_TIMEOUT,
        }
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Sends one request.
    ///
    /// # Errors
    /// [`HttpError`] when the device can't be reached or doesn't speak HTTP.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpError> {
        let work = self.exchange(method, path, body);
        match tokio::time::timeout(self.timeout, work).await {
            Ok(result) => result,
            Err(_) => Err(HttpError::Io(format!(
                "{} did not answer in {:?}",
                self.authority, self.timeout
            ))),
        }
    }

    async fn exchange(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpError> {
        let mut stream = TcpStream::connect(self.authority.as_str()).await?;
        let body = body.unwrap_or_default();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n",
            self.authority
        );
        if !body.is_empty() {
            let _ = write!(
                head,
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            );
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body).await?;
        let mut raw = Vec::new();
        let mut limited = (&mut stream).take(MAX_RESPONSE_BYTES as u64 + 1);
        limited.read_to_end(&mut raw).await?;
        if raw.len() > MAX_RESPONSE_BYTES {
            return Err(HttpError::Malformed("the response is too large".into()));
        }
        parse_response(&raw)
    }
}

fn with_port(authority: &str) -> String {
    let has_port = match authority.rsplit_once(':') {
        // An IPv6 literal such as `[::1]` has colons of its own
        Some((_, port)) => port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty(),
        None => false,
    };
    if has_port {
        authority.to_owned()
    } else {
        format!("{authority}:80")
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Reads a whole response (the connection was closed, so it is all there is).
///
/// # Errors
/// [`HttpError::Malformed`] when it isn't HTTP/1.x or its framing is wrong.
pub fn parse_response(raw: &[u8]) -> Result<HttpResponse, HttpError> {
    let malformed = |what: &str| HttpError::Malformed(what.to_owned());
    let split = find(raw, b"\r\n\r\n").ok_or_else(|| malformed("no end of headers"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| malformed("headers aren't text"))?;
    let rest = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    if !parts.next().is_some_and(|v| v.starts_with("HTTP/1.")) {
        return Err(malformed("not an HTTP/1.x status line"));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| malformed("no status code"))?;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse::<usize>().ok(),
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }
    let body = if chunked {
        dechunk(rest)?
    } else if let Some(length) = length {
        rest.get(..length)
            .ok_or_else(|| malformed("the body is shorter than Content-Length"))?
            .to_vec()
    } else {
        rest.to_vec()
    };
    Ok(HttpResponse { status, body })
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, HttpError> {
    let malformed = |what: &str| HttpError::Malformed(what.to_owned());
    let mut body = Vec::new();
    loop {
        let line_end = find(rest, b"\r\n").ok_or_else(|| malformed("a chunk has no size line"))?;
        let size_text = std::str::from_utf8(&rest[..line_end])
            .map_err(|_| malformed("a chunk size isn't text"))?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| malformed("bad chunk size"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        let chunk = rest
            .get(..size)
            .ok_or_else(|| malformed("a chunk is cut short"))?;
        body.extend_from_slice(chunk);
        rest = rest
            .get(size + 2..)
            .ok_or_else(|| malformed("a chunk has no end"))?;
    }
}
