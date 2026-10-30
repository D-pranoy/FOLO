//! `GET /system/backup` and `POST /system/restore` (RB-09 step 7,
//! docs/specs/client-api.md#backup-and-restore). The archive and the
//! replacement are in `restore`; this maps them to HTTP.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::library_api::refused;
use crate::node::Node;
use crate::restore;
use crate::server::error;

type Shared = State<Arc<Node>>;

pub async fn backup(State(node): Shared) -> Response {
    let work = Arc::clone(&node);
    match tokio::task::spawn_blocking(move || restore::backup(&work)).await {
        Ok(Ok((name, bytes))) => {
            let mut response = bytes.into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            );
            // The name holds only digits, `T`, `Z` and the fixed text
            if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
                headers.insert(header::CONTENT_DISPOSITION, value);
            }
            response
        }
        Ok(Err(failure)) => refused(&failure),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the backup failed",
        ),
    }
}

pub async fn restore(State(node): Shared, body: Result<Bytes, BytesRejection>) -> Response {
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
    let work = Arc::clone(&node);
    match tokio::task::spawn_blocking(move || restore::restore(&work, &bytes)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(failure)) => refused(&failure),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the restore failed",
        ),
    }
}
