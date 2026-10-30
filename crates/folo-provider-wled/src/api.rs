//! The device's JSON API as the provider uses it, behind a trait so tests can serve it from
//! memory (a paused clock and real sockets don't mix).

use folo_provider::{BoxFuture, ProviderError};
use serde_json::Value;

use crate::http::{HttpClient, HttpError, HttpResponse};

/// `GET` and `POST` on a WLED device's JSON API.
pub trait DeviceApi: Send + Sync {
    /// `GET path`, parsed as JSON.
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Value, ProviderError>>;

    /// `POST path` with a JSON body; the answer, parsed as JSON.
    fn post<'a>(
        &'a self,
        path: &'a str,
        body: &'a Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>>;

    /// The host (and port, if not 80) a person opens the device's own UI at.
    fn authority(&self) -> String;
}

/// The API over HTTP.
#[derive(Clone, Debug)]
pub struct HttpApi {
    client: HttpClient,
}

impl HttpApi {
    #[must_use]
    pub fn new(authority: &str) -> Self {
        Self {
            client: HttpClient::new(authority),
        }
    }
}

/// What a status means to the provider: 401 and 403 are a refusal (the PIN), other non-200
/// statuses are failures.
pub(crate) fn status_error(status: u16, path: &str) -> Option<ProviderError> {
    match status {
        200 => None,
        401 | 403 => Some(ProviderError::Refused(format!(
            "{path}: the device wants its settings PIN (HTTP {status})"
        ))),
        other => Some(ProviderError::Failed(format!("{path}: HTTP {other}"))),
    }
}

/// A response as JSON; no answer at all is `Unreachable`.
fn into_json(result: Result<HttpResponse, HttpError>, path: &str) -> Result<Value, ProviderError> {
    let response = result.map_err(|e| ProviderError::Unreachable(format!("{path}: {e}")))?;
    if let Some(error) = status_error(response.status, path) {
        return Err(error);
    }
    serde_json::from_slice(&response.body)
        .map_err(|e| ProviderError::Failed(format!("{path}: the answer isn't JSON: {e}")))
}

impl DeviceApi for HttpApi {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move { into_json(self.client.request("GET", path, None).await, path) })
    }

    fn post<'a>(
        &'a self,
        path: &'a str,
        body: &'a Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        Box::pin(async move {
            let bytes = body.to_string().into_bytes();
            into_json(self.client.request("POST", path, Some(&bytes)).await, path)
        })
    }

    fn authority(&self) -> String {
        self.client.authority().to_owned()
    }
}
