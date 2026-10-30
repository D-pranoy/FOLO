//! The content library's routes (RB-09, docs/specs/client-api.md#library):
//! packs, their items and previews, and the gallery listing. The library
//! itself is `folo-library`; this maps it to HTTP.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use folo_library::{ItemType, LibraryError};
use serde_json::{Value, json};

use crate::node::Node;
use crate::server::error;

type Shared = State<Arc<Node>>;
/// An answer refusing a request, boxed so results stay small.
type Refusal = Box<Response>;

/// The API's answer for a refused library operation.
fn refused(failure: &LibraryError) -> Response {
    if let LibraryError::Io { reason } = failure {
        log::error!("library: {reason}");
    }
    let status =
        StatusCode::from_u16(failure.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = json!({"error": {
        "code": failure.code(),
        "message": failure.to_string(),
        "details": failure.details(),
    }});
    (status, Json(body)).into_response()
}

/// Runs a library operation off the async threads: installs hash and
/// write up to 32 MB.
async fn blocking<T: Send + 'static>(
    node: &Arc<Node>,
    work: impl FnOnce(&folo_library::Library) -> Result<T, LibraryError> + Send + 'static,
) -> Result<T, Refusal> {
    let library = Arc::clone(&node.library);
    match tokio::task::spawn_blocking(move || work(&library)).await {
        Ok(result) => result.map_err(|e| Box::new(refused(&e))),
        Err(_) => Err(Box::new(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the library failed",
        ))),
    }
}

fn item_type(name: &str) -> Result<ItemType, Refusal> {
    ItemType::parse(name).ok_or_else(|| {
        Box::new(refused(&LibraryError::NotFound {
            what: format!("item type {name}"),
        }))
    })
}

pub async fn packs(State(node): Shared) -> Response {
    match blocking(&node, |library| Ok(library.packs())).await {
        Ok(packs) => Json(Value::Array(packs)).into_response(),
        Err(response) => *response,
    }
}

/// `POST /library/packs[?replace=true]` with a `.folopack`.
pub async fn install(
    State(node): Shared,
    Query(query): Query<Vec<(String, String)>>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let replace = match query.iter().rev().find(|(name, _)| name == "replace") {
        None => false,
        Some((_, value)) if value == "true" => true,
        Some((_, value)) if value == "false" => false,
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "bad_query",
                "replace is true or false",
            );
        }
    };
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "the body is larger than the limit",
            );
        }
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "bad_body",
                "the body can't be read",
            );
        }
    };
    match blocking(&node, move |library| library.install(&bytes, replace)).await {
        Ok(info) => (StatusCode::CREATED, Json(info)).into_response(),
        Err(response) => *response,
    }
}

pub async fn pack(State(node): Shared, Path(pack): Path<String>) -> Response {
    match blocking(&node, move |library| library.manifest(&pack)).await {
        Ok(manifest) => Json(manifest).into_response(),
        Err(response) => *response,
    }
}

pub async fn remove_pack(State(node): Shared, Path(pack): Path<String>) -> Response {
    match blocking(&node, move |library| library.remove(&pack)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => *response,
    }
}

pub async fn export(State(node): Shared, Path(pack): Path<String>) -> Response {
    let name = pack.replace('/', "-");
    match blocking(&node, move |library| library.export(&pack)).await {
        Ok(bytes) => {
            let mut response = bytes.into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            );
            // The name holds only [a-z0-9-], so it's a safe header value
            if let Ok(value) =
                HeaderValue::from_str(&format!("attachment; filename=\"{name}.folopack\""))
            {
                headers.insert(header::CONTENT_DISPOSITION, value);
            }
            response
        }
        Err(response) => *response,
    }
}

pub async fn item(
    State(node): Shared,
    Path((pack, kind, item)): Path<(String, String, String)>,
) -> Response {
    let kind = match item_type(&kind) {
        Ok(kind) => kind,
        Err(response) => return *response,
    };
    match blocking(&node, move |library| library.item(&pack, kind, &item)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => *response,
    }
}

pub async fn put_item(
    State(node): Shared,
    Path((pack, kind, item)): Path<(String, String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let kind = match item_type(&kind) {
        Ok(kind) => kind,
        Err(response) => return *response,
    };
    let value = match body.map(|b| serde_json::from_slice::<Value>(&b)) {
        Ok(Ok(value)) => value,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "the body is larger than the limit",
            );
        }
        _ => return error(StatusCode::BAD_REQUEST, "bad_body", "the body isn't JSON"),
    };
    match blocking(&node, move |library| {
        library.put_item(&pack, kind, &item, value)
    })
    .await
    {
        Ok(stored) => Json(stored).into_response(),
        Err(response) => *response,
    }
}

pub async fn delete_item(
    State(node): Shared,
    Path((pack, kind, item)): Path<(String, String, String)>,
) -> Response {
    let kind = match item_type(&kind) {
        Ok(kind) => kind,
        Err(response) => return *response,
    };
    match blocking(&node, move |library| {
        library.delete_item(&pack, kind, &item)
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => *response,
    }
}

/// A preview, from the node's origin: sandboxed, so an image a pack calls
/// a preview can't run script there.
pub async fn preview(State(node): Shared, Path((pack, file)): Path<(String, String)>) -> Response {
    match blocking(&node, move |library| library.preview(&pack, &file)).await {
        Ok((bytes, content_type)) => {
            let mut response = bytes.into_response();
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; sandbox"),
            );
            response
        }
        Err(response) => *response,
    }
}

/// `GET /library/items?type=<type>` (default `scenes`).
pub async fn items(State(node): Shared, Query(query): Query<Vec<(String, String)>>) -> Response {
    let name = query
        .iter()
        .rev()
        .find(|(name, _)| name == "type")
        .map_or("scenes", |(_, value)| value.as_str());
    let Some(kind) = ItemType::parse(name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_query",
            "type is palettes, reactions, looks, scenes or zone-templates",
        );
    };
    match blocking(&node, move |library| Ok(library.items(kind))).await {
        Ok(items) => Json(Value::Array(items)).into_response(),
        Err(response) => *response,
    }
}
