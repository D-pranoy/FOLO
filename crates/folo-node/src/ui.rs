//! The UI's files (RB-07 step 5, ADR-0035 D7): the build embedded in the
//! node by `build.rs`. Files under `assets/`, named by their content hash,
//! are cacheable for a year; every other file is `no-store`. The server
//! adds the Content Security Policy.

use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::server::error;

include!(concat!(env!("OUT_DIR"), "/ui_files.rs"));

/// What a node built without the UI shows at `/`.
const NOT_BUILT: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
    <title>FOLO</title></head><body><p>This FOLO node was built without its user interface.</p>\
    </body></html>\n";

/// The file at `path` of the embedded UI.
#[must_use]
pub fn serve(method: &Method, path: &str) -> Response {
    serve_from(FILES, method, path)
}

fn serve_from(
    files: &'static [(&'static str, &'static [u8])],
    method: &Method,
    path: &str,
) -> Response {
    let name = match path.trim_start_matches('/') {
        "" => "index.html",
        name => name,
    };
    // 404 before 405, as the request checks order them
    let bytes = match files.binary_search_by(|(file, _)| (*file).cmp(name)) {
        Ok(at) => files[at].1,
        Err(_) if files.is_empty() && name == "index.html" => NOT_BUILT.as_bytes(),
        Err(_) => return error(StatusCode::NOT_FOUND, "not_found", "no such file"),
    };
    if method != Method::GET && method != Method::HEAD {
        let mut refused = error(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "the UI's files take GET and HEAD",
        );
        refused
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET,HEAD"));
        return refused;
    }
    file_response(name, bytes)
}

fn file_response(name: &str, bytes: &'static [u8]) -> Response {
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    (
        [
            (header::CONTENT_TYPE, content_type(name)),
            (header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response()
}

/// The content type of a file, by its extension.
fn content_type(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, e)| e);
    match extension.to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILT: &[(&str, &[u8])] = &[
        ("assets/index-3f2a9c.js", b"console.log(1)"),
        ("favicon.svg", b"<svg/>"),
        ("index.html", b"<!doctype html>"),
    ];

    fn header_of(response: &Response, name: header::HeaderName) -> &str {
        response.headers()[name].to_str().unwrap()
    }

    #[test]
    fn index_is_uncacheable_and_hashed_assets_are_immutable() {
        let index = serve_from(BUILT, &Method::GET, "/");
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(header_of(&index, header::CACHE_CONTROL), "no-store");
        assert_eq!(
            header_of(&index, header::CONTENT_TYPE),
            "text/html; charset=utf-8"
        );
        let script = serve_from(BUILT, &Method::GET, "/assets/index-3f2a9c.js");
        assert_eq!(
            header_of(&script, header::CACHE_CONTROL),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            header_of(&script, header::CONTENT_TYPE),
            "text/javascript; charset=utf-8"
        );
        let icon = serve_from(BUILT, &Method::HEAD, "/favicon.svg");
        assert_eq!(header_of(&icon, header::CACHE_CONTROL), "no-store");
    }

    #[test]
    fn other_paths_and_methods_are_refused() {
        let missing = serve_from(BUILT, &Method::GET, "/zones");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let traversal = serve_from(BUILT, &Method::GET, "/../Cargo.toml");
        assert_eq!(traversal.status(), StatusCode::NOT_FOUND);
        let post = serve_from(BUILT, &Method::POST, "/");
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(header_of(&post, header::ALLOW), "GET,HEAD");
        let post_missing = serve_from(BUILT, &Method::POST, "/nope");
        assert_eq!(post_missing.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_build_without_the_ui_says_so() {
        let page = serve_from(&[], &Method::GET, "/");
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(header_of(&page, header::CACHE_CONTROL), "no-store");
        assert_eq!(
            serve_from(&[], &Method::GET, "/app.js").status(),
            StatusCode::NOT_FOUND
        );
    }
}
