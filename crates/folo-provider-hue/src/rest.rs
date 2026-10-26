//! The bridge's REST APIs over HTTPS: the old `/api` (config, pairing, software update status)
//! and CLIP v2 (`/clip/v2/resource/...`), plus the event stream.
//!
//! Limits (`docs/integrations/hue.md`): about three concurrent requests, roughly ten light
//! commands a second, and `429` means back off.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use folo_provider::ProviderError;
use futures_util::{Stream, StreamExt};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::endpoint::Endpoint;

/// The header that carries the application key.
pub const KEY_HEADER: &str = "hue-application-key";
/// The header of `/auth/v1`'s answer that carries the application id.
pub const APPLICATION_ID_HEADER: &str = "hue-application-id";

/// A request to the bridge failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestError {
    /// The bridge didn't answer, or the connection (or its certificate) failed.
    Unreachable(String),
    /// `401` or `403`: the application key isn't accepted.
    Unauthorized,
    /// `429` three times in a row.
    RateLimited,
    /// Any other status, with what the bridge said.
    Status { code: u16, message: String },
    /// The answer isn't what the bridge sends.
    Malformed(String),
}

impl fmt::Display for RestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(e) => f.write_str(e),
            Self::Unauthorized => f.write_str("the bridge doesn't accept the application key"),
            Self::RateLimited => f.write_str("the bridge is rate limiting requests (429)"),
            Self::Status { code, message } => write!(f, "the bridge answered {code}: {message}"),
            Self::Malformed(e) => write!(f, "unexpected answer from the bridge: {e}"),
        }
    }
}

impl std::error::Error for RestError {}

impl From<RestError> for ProviderError {
    fn from(value: RestError) -> Self {
        match value {
            RestError::Unreachable(m) => Self::Unreachable(m),
            RestError::Unauthorized => Self::Refused(value.to_string()),
            _ => Self::Failed(value.to_string()),
        }
    }
}

/// Limits the client works within.
#[derive(Clone, Copy, Debug)]
pub struct RestLimits {
    /// Requests in flight at once.
    pub concurrent: usize,
    /// A request that takes longer than this has failed.
    pub timeout: Duration,
    /// Connecting takes at most this long.
    pub connect_timeout: Duration,
    /// How long to wait after the first `429` when the bridge says nothing; it doubles.
    pub backoff: Duration,
}

impl Default for RestLimits {
    fn default() -> Self {
        Self {
            concurrent: 3,
            timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(3),
            backoff: Duration::from_secs(1),
        }
    }
}

/// A client of one bridge. It holds no credentials but the application key it is given, and
/// never writes it anywhere.
#[derive(Clone)]
pub struct BridgeClient {
    http: reqwest::Client,
    endpoint: Endpoint,
    app_key: Option<String>,
    limits: RestLimits,
    slots: Arc<Semaphore>,
}

impl fmt::Debug for BridgeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeClient")
            .field("endpoint", &self.endpoint)
            .field("authenticated", &self.app_key.is_some())
            .finish_non_exhaustive()
    }
}

/// How many times a `429` is waited out.
const RATE_LIMIT_TRIES: u32 = 3;

impl BridgeClient {
    /// A client that checks the bridge with `tls` (see [`crate::trust`]).
    ///
    /// # Errors
    /// [`ProviderError::Failed`] when the HTTP client can't be built.
    pub fn new(
        endpoint: Endpoint,
        tls: rustls::ClientConfig,
        app_key: Option<String>,
        limits: RestLimits,
    ) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(limits.connect_timeout)
            .pool_max_idle_per_host(limits.concurrent)
            .build()
            .map_err(|e| ProviderError::Failed(format!("setting up HTTPS failed: {e}")))?;
        Ok(Self {
            http,
            endpoint,
            app_key,
            limits,
            slots: Arc::new(Semaphore::new(limits.concurrent.max(1))),
        })
    }

    /// The same bridge with another application key.
    #[must_use]
    pub fn with_app_key(&self, app_key: Option<String>) -> Self {
        Self {
            app_key,
            ..self.clone()
        }
    }

    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint.base_url())
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let builder = self
            .http
            .request(method, self.url(path))
            .timeout(self.limits.timeout);
        match &self.app_key {
            Some(key) => builder.header(KEY_HEADER, key),
            None => builder,
        }
    }

    /// Sends a request, waiting out `429`, and returns the status, the headers and the body.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, reqwest::header::HeaderMap, Vec<u8>), RestError> {
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|_| RestError::Unreachable("the client is closed".into()))?;
        let mut wait = self.limits.backoff;
        for attempt in 1..=RATE_LIMIT_TRIES {
            let mut request = self.request(method.clone(), path);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await.map_err(unreachable)?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < RATE_LIMIT_TRIES {
                let told = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs);
                tokio::time::sleep(told.unwrap_or(wait)).await;
                wait *= 2;
                continue;
            }
            let headers = response.headers().clone();
            let bytes = response.bytes().await.map_err(unreachable)?;
            return Ok((status, headers, bytes.to_vec()));
        }
        Err(RestError::RateLimited)
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, RestError> {
        let (status, _, bytes) = self.send(method, path, body).await?;
        check_status(status, &bytes)?;
        serde_json::from_slice(&bytes).map_err(|e| RestError::Malformed(e.to_string()))
    }

    /// `GET /api/config`: what the bridge says of itself, without a key.
    ///
    /// # Errors
    /// [`RestError`] when the bridge can't be reached or answers something else.
    pub async fn public_config(&self) -> Result<Value, RestError> {
        self.json(Method::GET, "/api/config", None).await
    }

    /// `POST /api`: one pairing attempt. The bridge answers with a list of one result.
    ///
    /// # Errors
    /// [`RestError`] when the bridge can't be reached.
    pub async fn pair(&self, device_type: &str) -> Result<Value, RestError> {
        let body = json!({"devicetype": device_type, "generateclientkey": true});
        let (_, _, bytes) = self.send(Method::POST, "/api", Some(&body)).await?;
        serde_json::from_slice(&bytes).map_err(|e| RestError::Malformed(e.to_string()))
    }

    /// `GET /auth/v1`: the application id, which names this application to the bridge and is the
    /// DTLS identity.
    ///
    /// # Errors
    /// [`RestError::Unauthorized`] when the key isn't accepted.
    pub async fn application_id(&self) -> Result<String, RestError> {
        let (status, headers, bytes) = self.send(Method::GET, "/auth/v1", None).await?;
        check_status(status, &bytes)?;
        headers
            .get(APPLICATION_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| RestError::Malformed("no hue-application-id header".into()))
    }

    /// `GET /api/<key>/config` of the old API: the software update status is in it.
    ///
    /// # Errors
    /// [`RestError`] when the bridge can't be reached or the key isn't accepted.
    pub async fn authenticated_config(&self) -> Result<Value, RestError> {
        let key = self.app_key.as_deref().ok_or(RestError::Unauthorized)?;
        let value = self
            .json(Method::GET, &format!("/api/{key}/config"), None)
            .await?;
        // The old API answers 200 with an error list when the key is refused
        if value
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e.get("error").is_some()))
        {
            return Err(RestError::Unauthorized);
        }
        Ok(value)
    }

    /// `GET /clip/v2/resource/<resource>`: the `data` list.
    ///
    /// # Errors
    /// [`RestError`] when the request fails or the bridge reports errors.
    pub async fn resources(&self, resource: &str) -> Result<Vec<Value>, RestError> {
        let path = format!("/clip/v2/resource/{resource}");
        let answer = self.json(Method::GET, &path, None).await?;
        data_of(&answer)
    }

    /// `PUT /clip/v2/resource/<path>`.
    ///
    /// # Errors
    /// [`RestError`] when the request fails or the bridge reports errors.
    pub async fn put(&self, path: &str, body: &Value) -> Result<Vec<Value>, RestError> {
        let path = format!("/clip/v2/resource/{path}");
        data_of(&self.json(Method::PUT, &path, Some(body)).await?)
    }

    /// `POST /clip/v2/resource/<path>`.
    ///
    /// # Errors
    /// [`RestError`] when the request fails or the bridge reports errors.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Vec<Value>, RestError> {
        let path = format!("/clip/v2/resource/{path}");
        data_of(&self.json(Method::POST, &path, Some(body)).await?)
    }

    /// `DELETE /clip/v2/resource/<path>`.
    ///
    /// # Errors
    /// [`RestError`] when the request fails or the bridge reports errors.
    pub async fn delete(&self, path: &str) -> Result<Vec<Value>, RestError> {
        let path = format!("/clip/v2/resource/{path}");
        data_of(&self.json(Method::DELETE, &path, None).await?)
    }

    /// `PUT` with the retries the entertainment `start` and `stop` actions get: `tries` attempts,
    /// `pause` apart.
    ///
    /// # Errors
    /// The last [`RestError`].
    pub async fn put_retrying(
        &self,
        path: &str,
        body: &Value,
        tries: u32,
        pause: Duration,
    ) -> Result<Vec<Value>, RestError> {
        let mut last = RestError::Unreachable("no attempt was made".into());
        for attempt in 0..tries.max(1) {
            if attempt > 0 {
                tokio::time::sleep(pause).await;
            }
            match self.put(path, body).await {
                Ok(data) => return Ok(data),
                // A refused key or an unreachable bridge won't change in 500 ms
                Err(e @ (RestError::Unauthorized | RestError::Unreachable(_))) => return Err(e),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// `GET /eventstream/clip/v2`: the bridge's server-sent events, each message a list of events.
    ///
    /// # Errors
    /// [`RestError`] when the stream can't be opened.
    pub async fn events(
        &self,
    ) -> Result<impl Stream<Item = Result<Vec<Value>, RestError>> + use<>, RestError> {
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|_| RestError::Unreachable("the client is closed".into()))?;
        // No timeout: the stream is meant to stay open
        let mut request = self
            .http
            .get(self.url("/eventstream/clip/v2"))
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(key) = &self.app_key {
            request = request.header(KEY_HEADER, key);
        }
        let response = request.send().await.map_err(unreachable)?;
        check_status(response.status(), b"")?;
        let mut pending = Vec::new();
        Ok(response.bytes_stream().filter_map(move |chunk| {
            let out = match chunk {
                Err(e) => Some(Err(unreachable(e))),
                Ok(bytes) => {
                    pending.extend_from_slice(&bytes);
                    take_messages(&mut pending)
                }
            };
            std::future::ready(out)
        }))
    }
}

/// The events in the complete messages at the front of `pending`, which are removed from it.
/// `None` when there is no complete message yet.
fn take_messages(pending: &mut Vec<u8>) -> Option<Result<Vec<Value>, RestError>> {
    let mut events = Vec::new();
    while let Some(end) = find_blank_line(pending) {
        let message: Vec<u8> = pending.drain(..end.1).collect();
        let text = String::from_utf8_lossy(&message[..end.0]);
        let data: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect();
        if data.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&data.join("\n")) {
            Ok(Value::Array(list)) => events.extend(list),
            Ok(other) => events.push(other),
            Err(e) => return Some(Err(RestError::Malformed(e.to_string()))),
        }
    }
    (!events.is_empty()).then_some(Ok(events))
}

/// Where the first message ends: the length of its text and of the text with its blank line.
fn find_blank_line(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|i| (i, i + 2));
    let crlf = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, i + 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn unreachable(e: reqwest::Error) -> RestError {
    // The URL of an old-API request holds the application key
    RestError::Unreachable(error_chain(&e.without_url()))
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        text = format!("{text}: {inner}");
        source = inner.source();
    }
    text
}

fn check_status(status: StatusCode, body: &[u8]) -> Result<(), RestError> {
    if status.is_success() {
        return Ok(());
    }
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(RestError::Unauthorized),
        StatusCode::TOO_MANY_REQUESTS => Err(RestError::RateLimited),
        _ => Err(RestError::Status {
            code: status.as_u16(),
            message: errors_in(&serde_json::from_slice(body).unwrap_or(Value::Null))
                .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_owned()),
        }),
    }
}

/// The descriptions of a CLIP v2 answer's `errors`, if it has any.
fn errors_in(answer: &Value) -> Option<String> {
    let errors = answer.get("errors")?.as_array()?;
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(|e| e.get("description").and_then(Value::as_str))
        .collect();
    (!errors.is_empty()).then(|| messages.join("; "))
}

fn data_of(answer: &Value) -> Result<Vec<Value>, RestError> {
    if let Some(message) = errors_in(answer) {
        return Err(RestError::Status { code: 200, message });
    }
    match answer.get("data") {
        Some(Value::Array(list)) => Ok(list.clone()),
        _ => Err(RestError::Malformed("no data list".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_cut_at_blank_lines_and_may_arrive_in_pieces() {
        let mut pending =
            b": hi\n\nid: 1\ndata: [{\"type\":\"update\"}]\n\nid: 2\ndata: [{\"ty".to_vec();
        let out = take_messages(&mut pending).unwrap().unwrap();
        assert_eq!(out, vec![json!({"type": "update"})]);
        assert!(take_messages(&mut pending).is_none());
        pending.extend_from_slice(b"pe\":\"add\"}]\r\n\r\n");
        let out = take_messages(&mut pending).unwrap().unwrap();
        assert_eq!(out, vec![json!({"type": "add"})]);
        assert_eq!(pending, Vec::<u8>::new());
    }

    #[test]
    fn v2_errors_are_read_from_the_answer() {
        let answer = json!({"errors": [{"description": "link button"}], "data": []});
        assert_eq!(errors_in(&answer).as_deref(), Some("link button"));
        assert!(data_of(&answer).is_err());
        assert_eq!(
            data_of(&json!({"errors": [], "data": [1]})).unwrap(),
            vec![json!(1)]
        );
    }
}
