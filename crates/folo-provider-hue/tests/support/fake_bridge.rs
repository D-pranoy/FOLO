//! A Hue bridge on loopback: the old `/api` (config, pairing), `/auth/v1`, CLIP v2 resources and
//! entertainment configuration CRUD, the event stream, and a DTLS endpoint for the stream.

use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use folo_provider_hue::endpoint::Endpoint;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use super::certs::{self, Authority};
use super::dtls_server::DtlsServer;

pub const BRIDGE_ID: &str = "001788fffe23bfc2";
pub const USERNAME: &str = "test-application-key";
pub const CLIENT_KEY: &str = "00112233445566778899aabbccddeeff";
pub const APPLICATION_ID: &str = "b8e6c5d0-0f6a-4f8e-9a52-7c1d3a4e5f60";
pub const AREA: &str = "11111111-2222-3333-4444-555555555555";
pub const OTHER_APP: &str = "99999999-8888-7777-6666-555555555555";

/// Whose certificate the bridge presents.
#[derive(Clone, Copy, Debug)]
pub enum Certificate {
    /// Signed by the test authority, named for the bridge.
    Signed,
    /// Signed by the test authority, named for another bridge.
    WrongName,
    /// Signed by nobody, named for the bridge (an older firmware).
    SelfSigned,
}

pub struct State_ {
    pub api_version: String,
    pub model: String,
    /// Pairing answers "link button not pressed" until this many attempts were made.
    pub presses_after: u32,
    pub pair_attempts: u32,
    pub configurations: Vec<Value>,
    pub services: Vec<Value>,
    pub lights: Vec<Value>,
    pub devices: Vec<Value>,
    pub rooms: Vec<Value>,
    pub zones: Vec<Value>,
    /// What the bridge was asked: `METHOD path`, and the body.
    pub requests: Vec<(String, String, Value)>,
    pub identified: Vec<String>,
    /// The next `start` actions answer 500 this many times.
    pub start_failures: u32,
    /// The next requests answer 429 this many times.
    pub rate_limited: u32,
    next_area: u32,
    events: broadcast::Sender<String>,
    pub update_state: String,
}

pub type Shared = Arc<Mutex<State_>>;

fn rid(id: &str, rtype: &str) -> Value {
    json!({"rid": id, "rtype": rtype})
}

fn seed() -> (Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>) {
    let member = |service: &str, index: u32| json!([{"service": rid(service, "entertainment"), "index": index}]);
    let configuration = json!({
        "id": AREA, "type": "entertainment_configuration",
        "metadata": {"name": "TV"}, "configuration_type": "screen", "status": "inactive",
        "channels": [
            {"channel_id": 0, "members": member("svc-lamp", 0)},
            {"channel_id": 1, "members": member("svc-strip", 0)},
            {"channel_id": 2, "members": member("svc-strip", 1)},
            {"channel_id": 3, "members": member("svc-strip", 2)},
        ],
    });
    let services = vec![
        json!({"id": "svc-lamp", "type": "entertainment", "owner": rid("dev-lamp", "device"), "renderer": true}),
        json!({"id": "svc-strip", "type": "entertainment", "owner": rid("dev-strip", "device"), "renderer": true}),
    ];
    let lights = vec![
        json!({"id": "light-lamp", "type": "light", "owner": rid("dev-lamp", "device"), "metadata": {"name": "Desk lamp"}}),
        json!({"id": "light-strip", "type": "light", "owner": rid("dev-strip", "device"), "metadata": {"name": "Gradient strip"}, "gradient": {"points_capable": 5}}),
    ];
    let devices = vec![
        json!({"id": "dev-lamp", "type": "device", "metadata": {"name": "Desk lamp"}}),
        json!({"id": "dev-strip", "type": "device", "metadata": {"name": "Gradient strip"}}),
    ];
    (vec![configuration], services, lights, devices)
}

pub struct FakeBridge {
    pub endpoint: Endpoint,
    pub state: Shared,
    pub authority_der: Vec<u8>,
    pub certificate_der: Vec<u8>,
    pub dtls: DtlsServer,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeBridge {
    pub async fn start() -> Self {
        Self::with_certificate(Certificate::Signed).await
    }

    pub async fn with_certificate(kind: Certificate) -> Self {
        let authority = Authority::new("test-root-bridge");
        let (certificate_der, key_der) = match kind {
            Certificate::Signed => authority.issue(BRIDGE_ID),
            Certificate::WrongName => authority.issue("001788fffe000000"),
            Certificate::SelfSigned => certs::self_signed(BRIDGE_ID),
        };
        let (configurations, services, lights, devices) = seed();
        let (events, _) = broadcast::channel(16);
        let state: Shared = Arc::new(Mutex::new(State_ {
            api_version: "1.65.0".into(),
            model: "BSB002".into(),
            presses_after: 0,
            pair_attempts: 0,
            configurations,
            services,
            lights,
            devices,
            rooms: vec![json!({
                "id": "room-1", "metadata": {"name": "Living room", "archetype": "living_room"},
                "children": [rid("dev-lamp", "device")],
            })],
            zones: vec![json!({
                "id": "zone-1", "metadata": {"name": "Gradients", "archetype": "other"},
                "children": [rid("light-strip", "light")],
            })],
            requests: Vec::new(),
            identified: Vec::new(),
            start_failures: 0,
            rate_limited: 0,
            next_area: 0,
            events,
            update_state: "noupdates".into(),
        }));
        let dtls = DtlsServer::start(APPLICATION_ID, &hex::decode(CLIENT_KEY).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = acceptor(&certificate_der, &key_der);
        let router = router(Arc::clone(&state));
        let task = tokio::spawn(serve(listener, acceptor, router));
        Self {
            endpoint: Endpoint {
                host: "127.0.0.1".into(),
                https_port: port,
                dtls_port: dtls.port,
            },
            state,
            authority_der: authority.der(),
            certificate_der,
            dtls,
            task,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, State_> {
        self.state.lock().unwrap()
    }

    /// The requests made so far, `METHOD path`.
    pub fn requests(&self) -> Vec<String> {
        self.state()
            .requests
            .iter()
            .map(|(m, p, _)| format!("{m} {p}"))
            .collect()
    }

    /// The area as the bridge has it.
    pub fn area(&self, id: &str) -> Value {
        let state = self.state();
        state
            .configurations
            .iter()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap()
    }

    /// Another app starts streaming to the area.
    pub fn stream_from_another_app(&self) {
        let mut state = self.state();
        state.configurations[0]["status"] = json!("active");
        state.configurations[0]["active_streamer"] = rid(OTHER_APP, "auth_v1");
    }

    /// Sends an event to the event stream's readers.
    pub fn send_event(&self, events: &Value) {
        let _ = self.state().events.send(events.to_string());
    }
}

fn acceptor(certificate: &[u8], key: &[u8]) -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate.to_vec())],
            PrivatePkcs8KeyDer::from(key.to_vec()).into(),
        )
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

async fn serve(listener: TcpListener, acceptor: TlsAcceptor, router: Router) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let (acceptor, router) = (acceptor.clone(), router.clone());
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            let service = TowerToHyperService::new(
                router.map_request(|r: axum::http::Request<Incoming>| r.map(Body::new)),
            );
            let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .http1_only()
                .serve_connection(TokioIo::new(tls), service)
                .await;
        });
    }
}

fn router(state: Shared) -> Router {
    Router::new()
        .route("/api/config", get(public_config))
        .route("/api", axum::routing::post(pair))
        .route("/api/{user}/config", get(user_config))
        .route("/auth/v1", get(auth))
        .route("/discovery", get(discovery))
        .route("/eventstream/clip/v2", get(events))
        .route("/clip/v2/resource/{rtype}", get(list).post(create))
        .route(
            "/clip/v2/resource/{rtype}/{id}",
            axum::routing::put(update).delete(remove),
        )
        .with_state(state)
}

fn record(state: &Shared, method: &str, path: &str, body: &Value) -> Option<Response> {
    let mut s = state.lock().unwrap();
    s.requests
        .push((method.to_owned(), path.to_owned(), body.clone()));
    if s.rate_limited > 0 {
        s.rate_limited -= 1;
        let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("0"));
        return Some(response);
    }
    None
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("hue-application-key")
        .and_then(|v| v.to_str().ok())
        == Some(USERNAME)
}

fn data(list: &[Value]) -> Response {
    axum::Json(json!({"errors": [], "data": list})).into_response()
}

fn error(status: StatusCode, description: &str) -> Response {
    (
        status,
        axum::Json(json!({"errors": [{"description": description}], "data": []})),
    )
        .into_response()
}

async fn public_config(State(state): State<Shared>) -> Response {
    if let Some(limited) = record(&state, "GET", "/api/config", &Value::Null) {
        return limited;
    }
    let s = state.lock().unwrap();
    axum::Json(json!({
        "name": "Hue Bridge", "bridgeid": BRIDGE_ID.to_uppercase(), "modelid": s.model,
        "swversion": "1976154040", "apiversion": s.api_version, "mac": "00:17:88:23:bf:c2",
    }))
    .into_response()
}

async fn pair(State(state): State<Shared>, axum::Json(body): axum::Json<Value>) -> Response {
    if let Some(limited) = record(&state, "POST", "/api", &body) {
        return limited;
    }
    let mut s = state.lock().unwrap();
    s.pair_attempts += 1;
    if body.get("devicetype").is_none() {
        return axum::Json(
            json!([{"error": {"type": 2, "description": "body contains invalid json"}}]),
        )
        .into_response();
    }
    if s.pair_attempts <= s.presses_after {
        return axum::Json(json!([{"error": {"type": 101, "address": "", "description": "link button not pressed"}}])).into_response();
    }
    axum::Json(json!([{"success": {"username": USERNAME, "clientkey": CLIENT_KEY.to_uppercase()}}]))
        .into_response()
}

async fn user_config(State(state): State<Shared>, Path(user): Path<String>) -> Response {
    record(&state, "GET", "/api/<key>/config", &Value::Null);
    if user != USERNAME {
        return axum::Json(json!([{"error": {"type": 1, "description": "unauthorized user"}}]))
            .into_response();
    }
    let s = state.lock().unwrap();
    axum::Json(json!({
        "swversion": "1976154040", "apiversion": s.api_version,
        "swupdate2": {"state": s.update_state, "bridge": {"state": "noupdates", "lastinstall": "2025-01-01T00:00:00"}, "autoinstall": {"on": true, "updatetime": "T14:00:00"}},
    }))
    .into_response()
}

async fn auth(State(state): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(limited) = record(&state, "GET", "/auth/v1", &Value::Null) {
        return limited;
    }
    if !authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        "hue-application-id",
        HeaderValue::from_static(APPLICATION_ID),
    );
    response
}

async fn discovery() -> Response {
    axum::Json(json!([{"id": BRIDGE_ID, "internalipaddress": "127.0.0.1", "port": 443}]))
        .into_response()
}

async fn events(State(state): State<Shared>, headers: HeaderMap) -> Response {
    record(&state, "GET", "/eventstream/clip/v2", &Value::Null);
    if !authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut receiver = state.lock().unwrap().events.subscribe();
    let stream = async_stream(async move {
        let first = Ok::<_, Infallible>(axum::body::Bytes::from_static(b": hi\n\n"));
        let mut frames = vec![first];
        // The reader asks for a frame at a time; one event stream message per broadcast
        if let Ok(message) = receiver.recv().await {
            frames.push(Ok(axum::body::Bytes::from(format!(
                "id: 1:0\ndata: {message}\n\n"
            ))));
        }
        frames
    });
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

/// A body stream from a future that produces its frames: the hello comment goes out at once,
/// then the events as they come.
fn async_stream(
    future: impl std::future::Future<Output = Vec<Result<axum::body::Bytes, Infallible>>>
    + Send
    + 'static,
) -> impl futures_util::Stream<Item = Result<axum::body::Bytes, Infallible>> + Send + 'static {
    use futures_util::StreamExt;
    futures_util::stream::once(future).flat_map(futures_util::stream::iter)
}

fn resources(s: &State_, rtype: &str) -> Option<Vec<Value>> {
    Some(match rtype {
        "entertainment_configuration" => s.configurations.clone(),
        "entertainment" => s.services.clone(),
        "light" => s.lights.clone(),
        "device" => s.devices.clone(),
        "room" => s.rooms.clone(),
        "zone" => s.zones.clone(),
        _ => return None,
    })
}

async fn list(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(rtype): Path<String>,
) -> Response {
    if let Some(limited) = record(
        &state,
        "GET",
        &format!("/clip/v2/resource/{rtype}"),
        &Value::Null,
    ) {
        return limited;
    }
    if !authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match resources(&state.lock().unwrap(), &rtype) {
        Some(list) => data(&list),
        None => error(StatusCode::NOT_FOUND, "no such resource"),
    }
}

/// How many entertainment channels a service has: one per segment of a gradient light.
fn channels_of(s: &State_, service: &str) -> u32 {
    if service == "svc-strip" {
        3
    } else {
        u32::from(s.services.iter().any(|x| x["id"] == service))
    }
}

async fn create(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(rtype): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if let Some(limited) = record(&state, "POST", &format!("/clip/v2/resource/{rtype}"), &body) {
        return limited;
    }
    if !authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if rtype != "entertainment_configuration" {
        return error(StatusCode::BAD_REQUEST, "cannot create this resource");
    }
    let mut s = state.lock().unwrap();
    s.next_area += 1;
    let id = format!("aaaaaaaa-0000-0000-0000-{:012}", s.next_area);
    let mut channels = Vec::new();
    let mut next = 0u32;
    for location in body["locations"]["service_locations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let service = location["service"]["rid"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        for index in 0..channels_of(&s, &service) {
            channels.push(json!({"channel_id": next, "members": [{"service": rid(&service, "entertainment"), "index": index}]}));
            next += 1;
        }
    }
    s.configurations.push(json!({
        "id": id, "type": "entertainment_configuration", "metadata": body["metadata"],
        "configuration_type": body["configuration_type"], "status": "inactive", "channels": channels,
    }));
    data(&[rid(&id, "entertainment_configuration")])
}

async fn update(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path((rtype, id)): Path<(String, String)>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if let Some(limited) = record(
        &state,
        "PUT",
        &format!("/clip/v2/resource/{rtype}/{id}"),
        &body,
    ) {
        return limited;
    }
    if !authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut s = state.lock().unwrap();
    match rtype.as_str() {
        "entertainment_configuration" => update_area(&mut s, &id, &body),
        "light" => {
            if body.pointer("/identify/action") == Some(&json!("identify")) {
                s.identified.push(id.clone());
            }
            data(&[rid(&id, "light")])
        }
        "device" => {
            // Renaming a device renames its light
            let name = body["metadata"]["name"].clone();
            for device in s.devices.iter_mut().filter(|d| d["id"] == id.as_str()) {
                device["metadata"]["name"] = name.clone();
            }
            for light in s
                .lights
                .iter_mut()
                .filter(|l| l["owner"]["rid"] == id.as_str())
            {
                light["metadata"]["name"] = name.clone();
            }
            data(&[rid(&id, "device")])
        }
        _ => error(StatusCode::NOT_FOUND, "no such resource"),
    }
}

fn update_area(s: &mut State_, id: &str, body: &Value) -> Response {
    let me = rid(APPLICATION_ID, "auth_v1");
    let Some(index) = s.configurations.iter().position(|c| c["id"] == id) else {
        return error(StatusCode::NOT_FOUND, "no such area");
    };
    match body["action"].as_str() {
        Some("start") => {
            if s.start_failures > 0 {
                s.start_failures -= 1;
                return error(StatusCode::INTERNAL_SERVER_ERROR, "busy");
            }
            // Starting doesn't change the owner of a stream that already runs
            if s.configurations[index]["status"] != "active" {
                s.configurations[index]["status"] = json!("active");
                s.configurations[index]["active_streamer"] = me;
            }
        }
        Some("stop") => {
            s.configurations[index]["status"] = json!("inactive");
            s.configurations[index]
                .as_object_mut()
                .unwrap()
                .remove("active_streamer");
        }
        _ => {
            if let Some(name) = body.pointer("/metadata/name") {
                s.configurations[index]["metadata"]["name"] = name.clone();
            }
            if let Some(kind) = body.get("configuration_type") {
                s.configurations[index]["configuration_type"] = kind.clone();
            }
        }
    }
    data(&[rid(id, "entertainment_configuration")])
}

async fn remove(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path((rtype, id)): Path<(String, String)>,
) -> Response {
    if let Some(limited) = record(
        &state,
        "DELETE",
        &format!("/clip/v2/resource/{rtype}/{id}"),
        &Value::Null,
    ) {
        return limited;
    }
    if !authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut s = state.lock().unwrap();
    s.configurations.retain(|c| c["id"] != id.as_str());
    data(&[rid(&id, &rtype)])
}
