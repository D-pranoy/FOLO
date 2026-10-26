//! The HTTP client against the fake device, and the parsing of what devices send.

use std::sync::Arc;

use folo_provider::ProviderError;
use folo_provider_wled::http::{HttpClient, HttpError, parse_response};
use folo_provider_wled::testing::{FakeDevice, FakeHttpServer};
use folo_provider_wled::{DeviceApi, HttpApi};
use serde_json::json;

#[tokio::test]
async fn a_get_returns_the_json_document() {
    let server = FakeHttpServer::start(Arc::new(FakeDevice::fixture("rgb-strip"))).await;
    let api = HttpApi::new(&server.authority());
    let info = api.get("/json/info").await.unwrap();
    assert_eq!(info["name"], "WLED Kitchen");
}

#[tokio::test]
async fn a_post_sends_its_json_body() {
    let device = Arc::new(FakeDevice::fixture("rgb-strip"));
    let server = FakeHttpServer::start(device.clone()).await;
    let api = HttpApi::new(&server.authority());
    api.post("/json/state", &json!({"on": false, "bri": 10}))
        .await
        .unwrap();
    assert_eq!(
        device.posts("/json/state"),
        vec![json!({"on": false, "bri": 10})]
    );
}

#[tokio::test]
async fn chunked_answers_are_reassembled() {
    let server = FakeHttpServer::start_with(Arc::new(FakeDevice::fixture("segments")), true).await;
    let api = HttpApi::new(&server.authority());
    let state = api.get("/json/state").await.unwrap();
    assert_eq!(state["seg"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_pin_wall_is_a_refusal_and_a_missing_path_a_failure() {
    let device = Arc::new(FakeDevice::fixture("rgb-strip").with_pin("1"));
    let server = FakeHttpServer::start(device).await;
    let api = HttpApi::new(&server.authority());
    assert!(matches!(
        api.post("/json/cfg", &json!({"id": {"name": "x"}}))
            .await
            .unwrap_err(),
        ProviderError::Refused(_)
    ));
    assert!(matches!(
        api.get("/json/nothing").await.unwrap_err(),
        ProviderError::Failed(_)
    ));
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    let server = FakeHttpServer::start(Arc::new(FakeDevice::fixture("rgb-strip"))).await;
    let authority = server.authority();
    drop(server);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let error = HttpClient::new(&authority)
        .request("GET", "/json/info", None)
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::Io(_)));
}

#[test]
fn the_port_defaults_to_80() {
    assert_eq!(HttpClient::new("wled.local").authority(), "wled.local:80");
    assert_eq!(
        HttpClient::new("10.0.0.2:8080").authority(),
        "10.0.0.2:8080"
    );
    assert_eq!(HttpClient::new("[fe80::1]").authority(), "[fe80::1]:80");
    assert_eq!(HttpClient::new("[fe80::1]:81").authority(), "[fe80::1]:81");
}

#[test]
fn responses_are_read_by_content_length_chunks_or_the_close() {
    let sized = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcEXTRA").unwrap();
    assert_eq!((sized.status, sized.body.as_slice()), (200, &b"abc"[..]));

    let chunked = parse_response(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;ext=1\r\nde\r\n0\r\n\r\n",
    )
    .unwrap();
    assert_eq!(chunked.body, b"abcde");

    let until_close = parse_response(b"HTTP/1.0 401 Unauthorized\r\n\r\nno").unwrap();
    assert_eq!(
        (until_close.status, until_close.body.as_slice()),
        (401, &b"no"[..])
    );
}

#[test]
fn malformed_responses_are_refused() {
    for raw in [
        &b"garbage"[..],
        b"SMTP/1.1 200 OK\r\n\r\n",
        b"HTTP/1.1 OK\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab",
    ] {
        assert!(
            parse_response(raw).is_err(),
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
}
