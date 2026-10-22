//! The node's port from RB-06 on (ADR-0034 D6): the local link's endpoint
//! and the two loopback system routes, behind the client API's Host check
//! and its JSON rule. RB-07 builds the rest of the client API on this server.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{self, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use folo_supervisor::link::SUBPROTOCOL;
use folo_supervisor::{LinkSocket, Message};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::node::Node;
use crate::status;

/// The client API's body limit (docs/specs/client-api.md#conventions).
const BODY_LIMIT: usize = 1024 * 1024;
/// Host names always allowed by the Host check.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Serves `listener` until `stop` completes.
///
/// # Errors
///
/// When the server fails.
pub async fn serve(
    listener: TcpListener,
    node: Arc<Node>,
    stop: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        router(node).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stop)
    .await
}

fn router(node: Arc<Node>) -> Router {
    Router::new()
        .route("/api/v1/source-link", get(source_link))
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/reload", post(system_reload))
        // Only requests a route takes; the rest get their 404 or 405
        .route_layer(middleware::from_fn(json_only))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(Arc::clone(&node), guard))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(node)
}

/// An error in the client API's format.
fn error(status: StatusCode, code: &str, message: &str) -> Response {
    let body = json!({"error": {"code": code, "message": message, "details": {}}});
    (status, Json(body)).into_response()
}

/// The Host check, and loopback clients only (every route of RB-06 is
/// loopback only).
async fn guard(
    State(node): State<Arc<Node>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !host_allowed(&node, request.headers()) {
        return error(
            StatusCode::FORBIDDEN,
            "host_not_allowed",
            "the Host header isn't allowed",
        );
    }
    if !peer.ip().is_loopback() {
        return error(
            StatusCode::FORBIDDEN,
            "loopback_only",
            "only loopback clients are served",
        );
    }
    next.run(request).await
}

/// JSON for mutating requests (docs/specs/client-api.md#conventions).
async fn json_only(request: Request, next: Next) -> Response {
    let mutating = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if mutating && !is_json(request.headers()) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
            "mutating requests must send Content-Type: application/json",
        );
    }
    next.run(request).await
}

/// The loopback names, plus `remote_access.allowed_hosts` while remote
/// access is on (docs/architecture/security.md).
fn host_allowed(node: &Node, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let name = host_name(host);
    if LOOPBACK_HOSTS.contains(&name) {
        return true;
    }
    let settings = node.settings();
    settings.settings.remote_access.enabled
        && settings
            .settings
            .remote_access
            .allowed_hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// The name in a Host header, without its port or IPv6 brackets.
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "no such resource")
}

/// 405, with the `Allow` header axum adds (docs/specs/client-api.md#conventions).
async fn method_not_allowed() -> Response {
    error(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "the resource doesn't support this method",
    )
}

async fn system_status(State(node): State<Arc<Node>>) -> Json<Value> {
    Json(status::of_running(&node))
}

/// `POST /system/reload`; its body is `{}` (openapi.yaml).
async fn system_reload(State(node): State<Arc<Node>>, body: Bytes) -> Response {
    if !matches!(serde_json::from_slice::<Value>(&body), Ok(Value::Object(map)) if map.is_empty()) {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_body",
            "the body must be an empty JSON object",
        );
    }
    let node = Arc::clone(&node);
    match tokio::task::spawn_blocking(move || node.reload()).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(why)) => error(StatusCode::UNPROCESSABLE_ENTITY, "reload_failed", &why),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the reload failed",
        ),
    }
}

/// The local link's endpoint (docs/specs/source-link.md#the-local-link).
async fn source_link(State(node): State<Arc<Node>>, upgrade: WebSocketUpgrade) -> Response {
    let endpoint = node.supervisor.endpoint();
    upgrade
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |socket| async move { endpoint.serve(Socket(socket)).await })
}

/// axum's WebSocket as the link's socket.
struct Socket(WebSocket);

impl LinkSocket for Socket {
    async fn recv(&mut self) -> Option<Message> {
        loop {
            match self.0.recv().await? {
                Ok(ws::Message::Text(text)) => {
                    return Some(Message::Text(text.as_str().to_owned()));
                }
                Ok(ws::Message::Binary(bytes)) => return Some(Message::Binary(bytes.to_vec())),
                Ok(ws::Message::Ping(_) | ws::Message::Pong(_)) => {}
                Ok(ws::Message::Close(_)) | Err(_) => return None,
            }
        }
    }

    async fn send(&mut self, message: Message) -> bool {
        let message = match message {
            Message::Text(text) => ws::Message::Text(text.into()),
            Message::Binary(bytes) => ws::Message::Binary(bytes.into()),
        };
        self.0.send(message).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_lose_their_port_and_brackets() {
        assert_eq!(host_name("127.0.0.1:8777"), "127.0.0.1");
        assert_eq!(host_name("localhost"), "localhost");
        assert_eq!(host_name("[::1]:8777"), "::1");
        assert_eq!(host_name("[::1]"), "::1");
        assert_eq!(host_name("tv.local:80"), "tv.local");
    }

    #[test]
    fn json_is_recognised_with_parameters() {
        let mut headers = HeaderMap::new();
        assert!(!is_json(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert!(is_json(&headers));
        headers.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
        assert!(!is_json(&headers));
    }
}
