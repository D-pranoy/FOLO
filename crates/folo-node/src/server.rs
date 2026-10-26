//! The node's port (RB-07, ADR-0034 D6, ADR-0035): the client API, the
//! live channel, the local link's endpoint and the UI's files, behind the
//! request checks of docs/specs/client-api.md#request-checks.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{self, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{MethodRouter, get, post};
use folo_supervisor::link::SUBPROTOCOL;
use folo_supervisor::{LinkSocket, Message};
use serde_json::json;
use tokio::net::TcpListener;

use crate::node::Node;
use crate::{api, apply_api, backup_api, library_api, live, ui};

/// The client API's body limit (docs/specs/client-api.md#conventions).
const BODY_LIMIT: usize = 1024 * 1024;
/// A pack install's body limit: 10 MB (docs/specs/client-api.md#conventions).
const INSTALL_BODY_LIMIT: usize = 10 * 1024 * 1024;
/// A restore's body limit: 256 MB (docs/specs/client-api.md#conventions).
const RESTORE_BODY_LIMIT: usize = 256 * 1024 * 1024;
/// Mutating requests that send a zip archive instead of JSON (ADR-0038 D28).
/// `application/zip` isn't a type a page can send without a preflight, so
/// the JSON rule's protection holds.
const ZIP_ROUTES: [(Method, &str); 2] = [
    (Method::POST, "/api/v1/library/packs"),
    (Method::POST, "/api/v1/system/restore"),
];
/// Host names always allowed by the Host check.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
/// Endpoints other machines may call without a key: they use their own
/// proofs, or reveal nothing (ADR-0035 D1).
const WITHOUT_KEY: [&str; 4] = [
    "/api/v1/health",
    "/api/v1/source-link",
    "/api/v1/auth/login",
    "/api/v1/auth/requests",
];
/// The UI's Content Security Policy (docs/architecture/security.md#rules).
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
    style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; \
    font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
    frame-ancestors 'none'";

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

/// An endpoint whose subsystem arrives with `runbook` (ADR-0035 D2).
fn later(runbook: &'static str) -> impl Fn() -> std::future::Ready<Response> + Clone {
    move || std::future::ready(api::not_implemented(runbook))
}

fn later_get(runbook: &'static str) -> MethodRouter<Arc<Node>> {
    get(later(runbook))
}

fn later_post(runbook: &'static str) -> MethodRouter<Arc<Node>> {
    post(later(runbook))
}

/// The content library's paths (RB-09), apart for `router`'s length.
fn library_routes() -> Router<Arc<Node>> {
    let v1 = |path: &str| format!("/api/v1{path}");
    Router::new()
        .route(
            &v1("/library/packs"),
            get(library_api::packs)
                .post(post(library_api::install).layer(DefaultBodyLimit::max(INSTALL_BODY_LIMIT))),
        )
        .route(
            &v1("/library/packs/{pack}"),
            get(library_api::pack).delete(library_api::remove_pack),
        )
        .route(
            &v1("/library/packs/{pack}/export"),
            get(library_api::export),
        )
        .route(
            &v1("/library/packs/{pack}/items/{type}/{item}"),
            get(library_api::item)
                .put(library_api::put_item)
                .delete(library_api::delete_item),
        )
        .route(
            &v1("/library/packs/{pack}/previews/{file}"),
            get(library_api::preview),
        )
        .route(&v1("/library/items"), get(library_api::items))
}

/// Every path of specs/openapi.yaml, with the methods it lists.
pub fn router(node: Arc<Node>) -> Router {
    let v1 = |path: &str| format!("/api/v1{path}");
    Router::new()
        .route(&v1("/health"), get(api::health))
        .route(&v1("/info"), get(api::info))
        .route(&v1("/state"), get(api::state))
        .route(&v1("/zones"), get(api::zones).post(api::create_zone))
        .route(
            &v1("/zones/{id}"),
            get(api::zone).put(api::put_zone).delete(api::delete_zone),
        )
        .route(&v1("/zones/{id}/activate"), later_post("RB-08"))
        .route(&v1("/zones/{id}/deactivate"), later_post("RB-08"))
        .route(&v1("/zones/{id}/identify"), later_post("RB-08"))
        .route(&v1("/zones/{id}/test"), later_post("RB-08"))
        .route(&v1("/show"), get(api::show).put(api::put_show))
        .route(&v1("/show/reset"), post(apply_api::reset))
        .route(&v1("/scenes/{ref}/apply"), post(apply_api::apply))
        .route(&v1("/effects"), get(api::effects))
        .route(
            &v1("/providers"),
            get(api::providers).post(api::create_provider),
        )
        .route(
            &v1("/providers/{id}"),
            get(api::provider)
                .put(api::put_provider)
                .delete(api::delete_provider),
        )
        .route(&v1("/providers/discover"), later_post("RB-08"))
        .route(
            &v1("/providers/{id}/admin/{action}"),
            later_get("RB-08").post(later("RB-08")),
        )
        .route(&v1("/providers/{id}/hue/pair"), later_post("RB-13"))
        .route(&v1("/sources"), get(api::sources))
        .route(
            &v1("/sources/{id}"),
            get(api::source).put(api::put_source).delete(later("RB-16")),
        )
        .route(&v1("/sources/{id}/inputs"), get(api::source_inputs))
        .route(&v1("/sources/{id}/media"), get(api::source_media))
        .route(&v1("/sources/{id}/media/art"), get(api::source_media_art))
        .route(
            &v1("/sources/{id}/media/control"),
            post(api::source_media_control),
        )
        .route(&v1("/sources/invites"), later_post("RB-16"))
        .route(&v1("/sources/join-requests"), later_get("RB-16"))
        .route(
            &v1("/sources/join-requests/{id}/approve"),
            later_post("RB-16"),
        )
        .route(
            &v1("/sources/join-requests/{id}/reject"),
            later_post("RB-16"),
        )
        .merge(library_routes())
        .route(&v1("/clients"), later_get("RB-15").post(later("RB-15")))
        .route(
            &v1("/clients/{id}"),
            later_get("RB-15").delete(later("RB-15")),
        )
        .route(&v1("/updates"), later_get("RB-17"))
        .route(&v1("/updates/{node}/approve"), later_post("RB-17"))
        .route(&v1("/updates/{node}/skip"), later_post("RB-17"))
        .route(
            &v1("/updates/policy"),
            later_get("RB-17").put(later("RB-17")),
        )
        .route(&v1("/system"), get(api::system).put(later("RB-15")))
        .route(&v1("/system/status"), get(api::system_status))
        .route(&v1("/system/reload"), post(api::system_reload))
        .route(&v1("/system/logs"), get(api::system_logs))
        .route(&v1("/system/backup"), get(backup_api::backup))
        .route(
            &v1("/system/restore"),
            post(backup_api::restore).layer(DefaultBodyLimit::max(RESTORE_BODY_LIMIT)),
        )
        .route(&v1("/auth/login"), later_post("RB-15"))
        .route(&v1("/auth/logout"), later_post("RB-15"))
        .route(&v1("/auth/requests"), later_post("RB-15"))
        .route(&v1("/auth/requests/{id}/approve"), later_post("RB-15"))
        .route(&v1("/live"), get(live::live))
        .route(&v1("/ingest/surfaces"), later_get("RB-16"))
        .route(&v1("/source-link"), get(source_link))
        // Only requests a route takes; the rest get their 404 or 405
        .route_layer(middleware::from_fn(json_only))
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(Arc::clone(&node), guard))
        .layer(middleware::from_fn(headers))
        .with_state(node)
}

/// An error in the client API's format.
#[must_use]
pub fn error(status: StatusCode, code: &str, message: &str) -> Response {
    let body = json!({"error": {"code": code, "message": message, "details": {}}});
    (status, Json(body)).into_response()
}

/// `Cache-Control: no-store` on the API, the Content Security Policy on
/// everything else (the UI's files), and `nosniff` on every response.
async fn headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    } else {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        );
    }
    response
}

/// The Host and Origin checks, the client's address and authentication, in that order
/// (docs/specs/client-api.md#request-checks).
async fn guard(
    State(node): State<Arc<Node>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let remote_access = node.settings().settings.remote_access.enabled;
    if !host_allowed(&node, request.headers()) {
        return error(
            StatusCode::FORBIDDEN,
            "host_not_allowed",
            "the Host header isn't allowed",
        );
    }
    if !origin_allowed(&node, request.headers()) {
        return error(
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
            "requests from other sites aren't served",
        );
    }
    // A dual-stack listener sees 127.0.0.1 as ::ffff:127.0.0.1
    let ip = peer.ip().to_canonical();
    let loopback = ip.is_loopback();
    let served = loopback || (remote_access && private(ip));
    if !served {
        return error(
            StatusCode::FORBIDDEN,
            "client_not_allowed",
            "this client's address isn't served",
        );
    }
    let path = request.uri().path();
    if !loopback && path.starts_with("/api/") && !WITHOUT_KEY.contains(&path) {
        // Client keys and sessions arrive with RB-15; until then none is valid
        return error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a client key is needed",
        );
    }
    next.run(request).await
}

/// The private ranges remote access serves (docs/architecture/security.md#rules):
/// RFC 1918, link-local and unique-local IPv6.
fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

/// JSON for mutating requests (docs/specs/client-api.md#conventions), a zip
/// archive for the routes that take one.
async fn json_only(request: Request, next: Next) -> Response {
    let mutating = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    let zip = ZIP_ROUTES
        .iter()
        .any(|(method, path)| method == request.method() && *path == request.uri().path());
    if zip && !is_zip(request.headers()) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "zip_required",
            "this request must send Content-Type: application/zip",
        );
    }
    if mutating && !zip && !is_json(request.headers()) {
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
    name_allowed(node, host_name(host))
}

/// Whether a browser's `Origin` header, when it sends one, names the node
/// the way the Host check allows: a page on another site can't use the API
/// or the live channel through a loopback client (`null` origins included).
fn origin_allowed(node: &Node, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let authority = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default());
    authority.is_some_and(|a| !a.is_empty() && name_allowed(node, host_name(a)))
}

fn name_allowed(node: &Node, name: &str) -> bool {
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
    has_media_type(headers, "application/json")
}

fn is_zip(headers: &HeaderMap) -> bool {
    has_media_type(headers, "application/zip")
}

fn has_media_type(headers: &HeaderMap, wanted: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case(wanted))
}

/// Paths no route takes: 404 under `/api/`, the UI's files elsewhere.
async fn fallback(request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        return error(StatusCode::NOT_FOUND, "not_found", "no such resource");
    }
    ui::serve(request.method(), request.uri().path())
}

/// 405, with the `Allow` header axum adds (docs/specs/client-api.md#conventions).
async fn method_not_allowed() -> Response {
    error(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "the resource doesn't support this method",
    )
}

/// The source link's endpoint: the local link, from loopback clients only
/// (docs/specs/source-link.md#the-local-link, ADR-0035 D1).
async fn source_link(
    State(node): State<Arc<Node>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    upgrade: Result<WebSocketUpgrade, ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let loopback = peer.ip().to_canonical().is_loopback();
    if !loopback {
        // Only the local engine links before RB-16, so other machines are
        // refused before the upgrade, without buffering anything they send
        return error(
            StatusCode::FORBIDDEN,
            "loopback_only",
            "sources on other machines link with RB-16",
        );
    }
    let Ok(upgrade) = upgrade else {
        return not_a_websocket();
    };
    let endpoint = node.supervisor.endpoint();
    upgrade
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |socket| async move { endpoint.serve(Socket(socket), loopback).await })
}

/// A plain request to a WebSocket endpoint.
#[must_use]
pub fn not_a_websocket() -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "websocket_required",
        "this endpoint is a WebSocket",
    )
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

    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use tower::ServiceExt as _;

    /// A fresh data folder, removed on drop.
    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("folo-server-{}", folo_store::new_id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Sends `request` from `peer` to a node with `node_toml`'s settings;
    /// the answer's status, headers and body.
    async fn ask(
        node_toml: &str,
        peer: &str,
        request: Request,
    ) -> (StatusCode, HeaderMap, serde_json::Value) {
        let dir = Dir::new();
        let node = crate::node::for_tests(&dir.0, node_toml);
        let peer: SocketAddr = peer.parse().unwrap();
        let response = router(Arc::clone(&node))
            .layer(MockConnectInfo(peer))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        node.supervisor.shutdown().await;
        (status, headers, body)
    }

    fn get(path: &str, host: &str) -> Request {
        Request::builder()
            .uri(path)
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap()
    }

    const REMOTE: &str = "[remote_access]\nenabled = true\nallowed_hosts = [\"tv.local\"]\n";
    const LAN_PEER: &str = "192.168.1.20:50000";

    #[tokio::test]
    async fn other_machines_are_refused_while_remote_access_is_off() {
        let (status, _, body) = ask("", LAN_PEER, get("/api/v1/health", "localhost")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "client_not_allowed");
    }

    #[tokio::test]
    async fn with_remote_access_the_lan_needs_a_key_for_the_api() {
        // Health and the UI's files need no key; the rest of the API does,
        // and no key is valid before RB-15
        let (status, _, body) = ask(REMOTE, LAN_PEER, get("/api/v1/health", "tv.local:8777")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, headers, _) = ask(REMOTE, LAN_PEER, get("/", "tv.local:8777")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers[header::CONTENT_SECURITY_POLICY],
            CONTENT_SECURITY_POLICY
        );
        let (status, _, body) = ask(REMOTE, LAN_PEER, get("/api/v1/source-link", "tv.local")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "loopback_only");
        for path in ["/api/v1/state", "/api/v1/system/status", "/api/v1/live"] {
            let (status, headers, body) = ask(REMOTE, LAN_PEER, get(path, "tv.local")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            assert_eq!(body["error"]["code"], "unauthorized");
            assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        }
        // Addresses outside the LAN stay refused
        let (status, _, body) =
            ask(REMOTE, "8.8.8.8:50000", get("/api/v1/health", "tv.local")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "client_not_allowed");
        // Names outside the allowlist too, before anything else
        let (status, _, body) = ask(REMOTE, LAN_PEER, get("/api/v1/health", "evil.example")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "host_not_allowed");
    }

    #[tokio::test]
    async fn loopback_needs_no_key_and_every_response_is_marked() {
        let (status, headers, body) = ask(
            "",
            "[::ffff:127.0.0.1]:50000",
            get("/api/v1/info", "127.0.0.1:8777"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "test");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(headers.get(header::CONTENT_SECURITY_POLICY).is_none());
    }

    #[tokio::test]
    async fn pages_on_other_sites_are_refused() {
        let from = |origin: &str| {
            Request::builder()
                .uri("/api/v1/live")
                .header(header::HOST, "localhost:8777")
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap()
        };
        for origin in [
            "https://evil.example",
            "null",
            "http://localhost.evil.example",
        ] {
            let (status, _, body) = ask("", "127.0.0.1:50000", from(origin)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
            assert_eq!(body["error"]["code"], "origin_not_allowed");
        }
        // The node's own pages, by any of its names, pass the check
        let request = || {
            Request::builder()
                .uri("/api/v1/info")
                .header(header::HOST, "localhost:8777")
                .header(header::ORIGIN, "http://127.0.0.1:8777")
                .body(Body::empty())
                .unwrap()
        };
        let (status, _, _) = ask("", "127.0.0.1:50000", request()).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn later_subsystems_answer_501_with_their_run_book() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/zones/desk/activate")
            .header(header::HOST, "localhost")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let (status, _, body) = ask("", "127.0.0.1:50000", request).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["error"]["code"], "not_implemented");
        assert_eq!(body["error"]["details"]["runbook"], "RB-08");
    }

    #[test]
    fn private_ranges_are_the_lans() {
        for served in [
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.20",
            "169.254.3.4",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(private(served.parse().unwrap()), "{served}");
        }
        for refused in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "2001:db8::1"] {
            assert!(!private(refused.parse().unwrap()), "{refused}");
        }
    }
}
