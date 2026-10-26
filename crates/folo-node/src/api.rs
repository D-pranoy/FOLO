//! The client API's resources (RB-07 step 3, docs/specs/client-api.md): the
//! node, the documents, the effect catalog, requests to sources, the
//! system, and `501` for endpoints whose subsystems arrive later.

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use folo_protocol::clamp::{SCHEMA_VERSION, clamp};
use folo_store::{Doc, StoreError};
use folo_supervisor::config::LOCAL_SOURCE;
use folo_supervisor::{RequestError, RpcError};
use serde_json::{Value, json};

use crate::node::Node;
use crate::server::error;
use crate::status;

/// The client API's version: `info.version` in specs/openapi.yaml.
pub const CLIENT_API_VERSION: &str = "1.3.0";
/// JSON-RPC: what the request asked for isn't there (docs/specs/source-link.md).
const NOT_AVAILABLE: i64 = -32000;
/// JSON-RPC: params the source can't take.
const INVALID_PARAMS: i64 = -32602;
/// `GET /system/logs`: the default and the most lines (openapi.yaml).
const LOG_LINES: (u64, u64) = (500, 10_000);
/// The album art types `/media/art` serves (specs/openapi.yaml: `image/*`).
const ART_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
];

type Shared = State<Arc<Node>>;
type Body = Result<Bytes, BytesRejection>;
/// An answer refusing a request, boxed so results stay small.
type Refusal = Box<Response>;

static EFFECTS: LazyLock<Value> = LazyLock::new(|| Value::Array(folo_protocol::catalog::effects()));

pub async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

fn info_of(node: &Node) -> Value {
    json!({
        "id": node.id,
        "name": node.name,
        "version": env!("CARGO_PKG_VERSION"),
        "role": node.role,
        "contracts": {
            "client_api": CLIENT_API_VERSION,
            "source_link": folo_supervisor::link::VERSIONS[0],
            "config": SCHEMA_VERSION,
        },
        "capabilities": [],
    })
}

pub async fn info(State(node): Shared) -> Json<Value> {
    Json(info_of(&node))
}

pub async fn effects() -> Json<Value> {
    Json(EFFECTS.clone())
}

/// `GET /system`, before the desktop app and the service exist
/// (docs/specs/client-api.md#before-their-run-books).
fn system_of(node: &Node) -> Value {
    let settings = node.settings();
    let remote = &settings.settings.remote_access;
    let service = node.place.service;
    json!({
        "remote_access": {"enabled": remote.enabled, "allowed_hosts": remote.allowed_hosts},
        "start_at_sign_in": false,
        "service": {"installed": service, "running": service},
    })
}

pub async fn system(State(node): Shared) -> Json<Value> {
    Json(system_of(&node))
}

pub async fn state(State(node): Shared) -> Json<Value> {
    let doc = |doc: Doc| (*node.store.get(doc)).clone();
    Json(json!({
        "info": info_of(&node),
        "zones": doc(Doc::Zones),
        "show": doc(Doc::Show),
        "providers": doc(Doc::Providers),
        "sources": doc(Doc::Sources),
        "effects": EFFECTS.clone(),
        "system": system_of(&node),
    }))
}

// ---------------------------------------------------------------- later subsystems

/// An endpoint whose subsystem arrives with `runbook` (ADR-0035 D2).
#[must_use]
pub fn not_implemented(runbook: &'static str) -> Response {
    let body = json!({"error": {
        "code": "not_implemented",
        "message": format!("this arrives with {runbook}"),
        "details": {"runbook": runbook},
    }});
    (StatusCode::NOT_IMPLEMENTED, Json(body)).into_response()
}

// ---------------------------------------------------------------- the system

fn loopback_only(peer: SocketAddr) -> Option<Response> {
    (!peer.ip().to_canonical().is_loopback()).then(|| {
        error(
            StatusCode::FORBIDDEN,
            "loopback_only",
            "only loopback clients are served",
        )
    })
}

pub async fn system_status(
    State(node): Shared,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if let Some(refused) = loopback_only(peer) {
        return refused;
    }
    Json(status::of_running(&node)).into_response()
}

/// `POST /system/reload`; its body is `{}` (openapi.yaml).
pub async fn system_reload(
    State(node): Shared,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Body,
) -> Response {
    if let Some(refused) = loopback_only(peer) {
        return refused;
    }
    match read_json(body) {
        Ok(Value::Object(map)) if map.is_empty() => {}
        Ok(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "bad_body",
                "the body must be an empty JSON object",
            );
        }
        Err(refused) => return *refused,
    }
    let reloading = Arc::clone(&node);
    match tokio::task::spawn_blocking(move || reloading.reload()).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(why)) => error(StatusCode::UNPROCESSABLE_ENTITY, "reload_failed", &why),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the reload failed",
        ),
    }
}

pub async fn system_logs(
    State(node): Shared,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let lines = query.iter().rev().find(|(name, _)| name == "lines");
    let lines = match lines.map(|(_, value)| value) {
        None => LOG_LINES.0,
        Some(text) => match text.parse::<u64>() {
            Ok(n) if (1..=LOG_LINES.1).contains(&n) => n,
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "bad_query",
                    "lines must be a whole number from 1 to 10000",
                );
            }
        },
    };
    let path = node.place.folders.log_file();
    let count = usize::try_from(lines).unwrap_or(usize::MAX);
    match tokio::task::spawn_blocking(move || crate::logs::tail(&path, count)).await {
        Ok(lines) => Json(json!({"lines": lines})).into_response(),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the log can't be read",
        ),
    }
}

// ---------------------------------------------------------------- documents

/// A body as JSON, or the answer refusing it.
fn read_json(body: Body) -> Result<Value, Refusal> {
    let bytes = body.map_err(|rejection| {
        Box::new(if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "the body is larger than the limit",
            )
        } else {
            error(
                StatusCode::BAD_REQUEST,
                "bad_body",
                "the body can't be read",
            )
        })
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        Box::new(error(
            StatusCode::BAD_REQUEST,
            "bad_body",
            "the body isn't JSON",
        ))
    })
}

/// Why a document write was refused.
enum Refused {
    Store(StoreError),
    /// `If-Match` named another revision than the document's.
    Stale(u64),
    NotFound,
    /// The list holds as many items as its schema allows.
    Full,
}

impl From<StoreError> for Refused {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl Refused {
    fn response(self) -> Response {
        match self {
            Self::Store(StoreError::Structural { why, .. }) => {
                error(StatusCode::UNPROCESSABLE_ENTITY, "structural_error", &why)
            }
            Self::Store(StoreError::Io { error: why, .. }) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                &why.to_string(),
            ),
            Self::Stale(revision) => {
                let body = json!({"error": {
                    "code": "revision_mismatch",
                    "message": "the document changed since that revision",
                    "details": {"revision": revision},
                }});
                (StatusCode::PRECONDITION_FAILED, Json(body)).into_response()
            }
            Self::NotFound => error(StatusCode::NOT_FOUND, "not_found", "no such item"),
            Self::Full => error(
                StatusCode::CONFLICT,
                "list_full",
                "the list holds as many items as it may",
            ),
        }
    }
}

/// What `If-Match` asks for.
#[derive(Debug, PartialEq, Eq)]
enum IfMatch {
    /// No `If-Match`: the last writer wins.
    Absent,
    /// The revision the writer last saw, as `12` or `"12"`.
    Revision(u64),
    /// Anything else, which matches no revision.
    Unreadable,
}

fn if_match(headers: &HeaderMap) -> IfMatch {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return IfMatch::Absent;
    };
    let text = value.to_str().unwrap_or_default().trim();
    let text = text.strip_prefix("W/").unwrap_or(text).trim_matches('"');
    text.parse().map_or(IfMatch::Unreadable, IfMatch::Revision)
}

/// Refuses a write when `If-Match` names another revision than `current`'s.
fn check_revision(headers: &HeaderMap, current: &Value) -> Result<(), Refused> {
    let revision = current["revision"].as_u64().unwrap_or(0);
    match if_match(headers) {
        IfMatch::Absent => Ok(()),
        IfMatch::Revision(seen) if seen == revision => Ok(()),
        _ => Err(Refused::Stale(revision)),
    }
}

/// The items of a list document.
#[derive(Clone, Copy)]
struct Items {
    doc: Doc,
    /// The list's key in the document.
    key: &'static str,
    /// The schema definition of one item.
    definition: &'static str,
}

const ZONES: Items = Items {
    doc: Doc::Zones,
    key: "zones",
    definition: "Zone",
};
const PROVIDERS: Items = Items {
    doc: Doc::Providers,
    key: "providers",
    definition: "ProviderInstance",
};
const SOURCES: Items = Items {
    doc: Doc::Sources,
    key: "sources",
    definition: "Source",
};

/// How many items the list may hold (its schema's `maxItems`).
fn room(items: Items) -> usize {
    folo_protocol::clamp::definition(items.doc.definition())
        .and_then(|schema| schema["properties"][items.key]["maxItems"].as_u64())
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(usize::MAX)
}

fn list(document: &Value, items: Items) -> &[Value] {
    document[items.key].as_array().map_or(&[], Vec::as_slice)
}

fn find<'a>(document: &'a Value, items: Items, id: &str) -> Option<&'a Value> {
    list(document, items).iter().find(|item| item["id"] == id)
}

/// The document with `list` as its items.
fn with_list(document: &Value, items: Items, list: Vec<Value>) -> Value {
    let mut out = document.clone();
    out[items.key] = Value::Array(list);
    out
}

/// After a write: the engine gets its new configuration when the document
/// is one it's built from.
fn written(node: &Node, doc: Doc) {
    if matches!(doc, Doc::Zones | Doc::Show | Doc::Sources) {
        node.refresh_engine();
    }
}

fn get_document(node: &Node, doc: Doc) -> Json<Value> {
    Json((*node.store.get(doc)).clone())
}

fn get_item(node: &Node, items: Items, id: &str) -> Response {
    let document = node.store.get(items.doc);
    match find(&document, items, id) {
        Some(item) => Json(item.clone()).into_response(),
        None => Refused::NotFound.response(),
    }
}

/// `POST`: adds an item; an id that is missing, empty or taken gets a new one.
fn create_item(node: &Node, items: Items, body: Body) -> Response {
    let body = match read_json(body) {
        Ok(body) => body,
        Err(refused) => return *refused,
    };
    let mut id = String::new();
    let stored = node.store.update(items.doc, |document| {
        let mut item = clamp(items.definition, &body, &mut folo_store::new_id).map_err(|e| {
            StoreError::Structural {
                file: items.doc.file().into(),
                why: e.to_string(),
            }
        })?;
        let taken = |id: &Value| list(document, items).iter().any(|i| &i["id"] == id);
        while taken(&item["id"]) {
            item["id"] = Value::String(folo_store::new_id());
        }
        item["id"].as_str().unwrap_or_default().clone_into(&mut id);
        let mut all = list(document, items).to_vec();
        if all.len() >= room(items) {
            return Err(Refused::Full);
        }
        all.push(item);
        Ok::<_, Refused>(with_list(document, items, all))
    });
    match stored {
        Ok(document) => {
            written(node, items.doc);
            let item = find(&document, items, &id).cloned().unwrap_or(Value::Null);
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(refused) => refused.response(),
    }
}

/// `PUT`: replaces the item with that id, which keeps the id in the path.
/// `add` adds it when there is none.
fn put_item(
    node: &Node,
    items: Items,
    id: &str,
    headers: &HeaderMap,
    body: Body,
    add: bool,
) -> Response {
    let body = match read_json(body) {
        Ok(body) => body,
        Err(refused) => return *refused,
    };
    let stored = node.store.update(items.doc, |document| {
        check_revision(headers, document)?;
        // Clamped as an item first, so a body that isn't one is refused
        // rather than dropped from the list with the item it replaces
        let mut body = clamp(items.definition, &body, &mut folo_store::new_id).map_err(|e| {
            StoreError::Structural {
                file: items.doc.file().into(),
                why: e.to_string(),
            }
        })?;
        body["id"] = Value::String(id.to_owned());
        let mut all = list(document, items).to_vec();
        match all.iter().position(|item| item["id"] == id) {
            Some(at) => all[at] = body,
            None if add && all.len() >= room(items) => return Err(Refused::Full),
            None if add => all.push(body),
            None => return Err(Refused::NotFound),
        }
        Ok(with_list(document, items, all))
    });
    match stored {
        Ok(document) => {
            written(node, items.doc);
            let item = find(&document, items, id).cloned().unwrap_or(Value::Null);
            Json(item).into_response()
        }
        Err(refused) => refused.response(),
    }
}

fn delete_item(node: &Node, items: Items, id: &str) -> Response {
    let stored = node.store.update(items.doc, |document| {
        let all = list(document, items);
        if !all.iter().any(|item| item["id"] == id) {
            return Err(Refused::NotFound);
        }
        let kept = all
            .iter()
            .filter(|item| item["id"] != id)
            .cloned()
            .collect();
        Ok(with_list(document, items, kept))
    });
    match stored {
        Ok(_) => {
            written(node, items.doc);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(refused) => refused.response(),
    }
}

pub async fn zones(State(node): Shared) -> Json<Value> {
    get_document(&node, Doc::Zones)
}

pub async fn create_zone(State(node): Shared, body: Body) -> Response {
    create_item(&node, ZONES, body)
}

pub async fn zone(State(node): Shared, Path(id): Path<String>) -> Response {
    get_item(&node, ZONES, &id)
}

pub async fn put_zone(
    State(node): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    put_item(&node, ZONES, &id, &headers, body, false)
}

pub async fn delete_zone(State(node): Shared, Path(id): Path<String>) -> Response {
    delete_item(&node, ZONES, &id)
}

pub async fn show(State(node): Shared) -> Json<Value> {
    get_document(&node, Doc::Show)
}

pub async fn put_show(State(node): Shared, headers: HeaderMap, body: Body) -> Response {
    let body = match read_json(body) {
        Ok(body) => body,
        Err(refused) => return *refused,
    };
    let stored = node.store.update(Doc::Show, |document| {
        check_revision(&headers, document)?;
        Ok::<_, Refused>(body)
    });
    match stored {
        Ok(document) => {
            written(&node, Doc::Show);
            Json((*document).clone()).into_response()
        }
        Err(refused) => refused.response(),
    }
}

pub async fn providers(State(node): Shared) -> Json<Value> {
    get_document(&node, Doc::Providers)
}

pub async fn create_provider(State(node): Shared, body: Body) -> Response {
    create_item(&node, PROVIDERS, body)
}

pub async fn provider(State(node): Shared, Path(id): Path<String>) -> Response {
    get_item(&node, PROVIDERS, &id)
}

pub async fn put_provider(
    State(node): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    put_item(&node, PROVIDERS, &id, &headers, body, false)
}

pub async fn delete_provider(State(node): Shared, Path(id): Path<String>) -> Response {
    delete_item(&node, PROVIDERS, &id)
}

// ---------------------------------------------------------------- sources

pub async fn sources(State(node): Shared) -> Json<Value> {
    get_document(&node, Doc::Sources)
}

/// The source `local` is the node's own engine while the node runs it,
/// even when `sources.json` doesn't list it (ADR-0035 D8).
fn local_runs(node: &Node, id: &str) -> bool {
    id == LOCAL_SOURCE && node.engine_runs()
}

pub async fn source(State(node): Shared, Path(id): Path<String>) -> Response {
    let document = node.store.get(Doc::Sources);
    if let Some(source) = find(&document, SOURCES, &id) {
        return Json(source.clone()).into_response();
    }
    if local_runs(&node, &id) {
        let local = json!({"id": LOCAL_SOURCE});
        let defaults = clamp("Source", &local, &mut String::new).unwrap_or(local);
        return Json(defaults).into_response();
    }
    Refused::NotFound.response()
}

pub async fn put_source(
    State(node): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let add = local_runs(&node, &id);
    put_item(&node, SOURCES, &id, &headers, body, add)
}

/// Whether a request can go to the source `id`: only the local engine
/// links before RB-16.
fn reachable(node: &Node, id: &str) -> Result<(), Refusal> {
    if local_runs(node, id) {
        return Ok(());
    }
    if find(&node.store.get(Doc::Sources), SOURCES, id).is_some() {
        return Err(Box::new(offline()));
    }
    Err(Box::new(error(
        StatusCode::NOT_FOUND,
        "not_found",
        "no such source",
    )))
}

fn offline() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "source_offline",
        "the source isn't linked",
    )
}

/// An answer to a failed request to a source (docs/specs/client-api.md#requests-to-sources).
fn request_failed(failure: &RequestError, not_available: StatusCode) -> Response {
    match failure {
        RequestError::Offline => offline(),
        RequestError::Timeout => error(
            StatusCode::GATEWAY_TIMEOUT,
            "source_timeout",
            "the source didn't answer",
        ),
        RequestError::Rpc(RpcError { code, message }) => match *code {
            NOT_AVAILABLE => error(not_available, "not_available", message),
            INVALID_PARAMS => error(StatusCode::BAD_REQUEST, "bad_body", message),
            _ => error(StatusCode::BAD_GATEWAY, "source_error", message),
        },
    }
}

pub async fn source_inputs(State(node): Shared, Path(id): Path<String>) -> Response {
    if let Err(refused) = reachable(&node, &id) {
        return *refused;
    }
    match node.supervisor.request("inputs.list", json!({})).await {
        Ok(inputs) => Json(with_screens(&node, inputs).await).into_response(),
        Err(failure) => request_failed(&failure, StatusCode::NOT_FOUND),
    }
}

/// The engine leaves `screens` empty: the capture agent's list goes in its
/// place (ADR-0037 D15), from the linked agent or `folo-capture screens
/// --json`. A list that can't be had leaves it empty.
pub async fn with_screens(node: &Node, mut inputs: Value) -> Value {
    match node.capture.screens().await {
        Ok(listed) => inputs["screens"] = listed["screens"].clone(),
        Err(why) => log::debug!("no screen list: {why:?}"),
    }
    inputs
}

/// The source's latest `media` notification, while it's linked.
fn media_of(node: &Node) -> Option<Value> {
    if !node.supervisor.linked() {
        return None;
    }
    node.supervisor
        .received(|r| r.notifications.get("media").map(|m| m.value.clone()))
}

pub async fn source_media(State(node): Shared, Path(id): Path<String>) -> Response {
    if let Err(refused) = reachable(&node, &id) {
        return *refused;
    }
    media_of(&node).map_or_else(offline, |media| Json(media).into_response())
}

pub async fn source_media_art(State(node): Shared, Path(id): Path<String>) -> Response {
    if let Err(refused) = reachable(&node, &id) {
        return *refused;
    }
    let Some(media) = media_of(&node) else {
        return offline();
    };
    let art_id = media["art"].as_u64().unwrap_or(0);
    if art_id == 0 {
        return error(StatusCode::NOT_FOUND, "not_available", "no album art");
    }
    let art = match node
        .supervisor
        .request("media.art", json!({"art_id": art_id}))
        .await
    {
        Ok(art) => art,
        Err(failure) => return request_failed(&failure, StatusCode::NOT_FOUND),
    };
    let Ok(bytes) = STANDARD.decode(art["data"].as_str().unwrap_or_default()) else {
        return error(
            StatusCode::BAD_GATEWAY,
            "source_error",
            "the art isn't base64",
        );
    };
    // Served from the node's origin: only image types, and sandboxed, so art
    // a player labelled as a page can't run script there
    let content_type = art["content_type"].as_str().unwrap_or_default();
    if !ART_TYPES.contains(&content_type.to_ascii_lowercase().as_str()) {
        return error(
            StatusCode::BAD_GATEWAY,
            "source_error",
            "the art isn't an image",
        );
    }
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

pub async fn source_media_control(
    State(node): Shared,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    let body = match read_json(body) {
        Ok(body) => body,
        Err(refused) => return *refused,
    };
    let action = body.as_object().filter(|b| b.len() == 1).and_then(|b| {
        b.get("action")
            .and_then(Value::as_str)
            .filter(|a| ["play_pause", "next", "previous"].contains(a))
    });
    let Some(action) = action else {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_body",
            "the body must be {\"action\": \"play_pause\" | \"next\" | \"previous\"}",
        );
    };
    if let Err(refused) = reachable(&node, &id) {
        return *refused;
    }
    match node
        .supervisor
        .request("media.control", json!({"action": action}))
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => request_failed(&failure, StatusCode::CONFLICT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_match_takes_plain_and_quoted_revisions() {
        let mut headers = HeaderMap::new();
        assert_eq!(if_match(&headers), IfMatch::Absent);
        for (text, revision) in [("12", 12), ("\"12\"", 12), ("W/\"7\"", 7)] {
            headers.insert(header::IF_MATCH, text.parse().unwrap());
            assert_eq!(if_match(&headers), IfMatch::Revision(revision), "{text}");
        }
        headers.insert(header::IF_MATCH, "*".parse().unwrap());
        assert_eq!(if_match(&headers), IfMatch::Unreadable);
        let current = json!({"revision": 3});
        assert!(matches!(
            check_revision(&headers, &current),
            Err(Refused::Stale(3))
        ));
        headers.insert(header::IF_MATCH, "3".parse().unwrap());
        assert!(check_revision(&headers, &current).is_ok());
    }

    #[test]
    fn lists_have_the_room_their_schemas_give() {
        for items in [ZONES, PROVIDERS, SOURCES] {
            assert_eq!(room(items), 64, "{}", items.key);
        }
    }

    #[test]
    fn the_client_api_version_is_the_contracts() {
        let openapi = include_str!("../../../specs/openapi.yaml");
        assert!(
            openapi.contains(&format!("\n  version: {CLIENT_API_VERSION}\n")),
            "openapi.yaml's info.version"
        );
    }
}
