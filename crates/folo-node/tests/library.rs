//! The content library's routes end to end, in this process (RB-09 change
//! 3): every route and error of docs/specs/client-api.md#library, the zip
//! routes' exemption from the JSON rule, percent-encoded ids, and the live
//! channel's library events.

#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::format_push_string,
    clippy::cast_possible_truncation
)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener as StdListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use folo_library::archive::{self, Entries};
use folo_library::canonical::hex;
use folo_node::cli::{LocationArgs, RunArgs};
use folo_protocol::generated::{LibraryItem, LiveEventMessage, PackInfo, PackManifest};
use futures_util::StreamExt as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-library-api-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Node {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<u8>,
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap().to_owned()
    }

    fn details(&self) -> Value {
        self.json()["error"]["details"].clone()
    }
}

fn exchange(
    port: u16,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Answer {
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if !body.is_empty() || matches!(method, "POST" | "PUT") {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut raw = head.into_bytes();
    raw.extend_from_slice(body);
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    // A refused body may be answered before it is all read
    let _ = stream.write_all(&raw);
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(response[..split].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut body = response[split + 4..].to_vec();
    if headers
        .iter()
        .any(|(n, v)| n.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"))
    {
        body = dechunk(&body);
    }
    Answer {
        status,
        headers,
        body,
    }
}

fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let end = body.windows(2).position(|w| w == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&body[..end]).unwrap().trim(), 16).unwrap();
        body = &body[end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

impl Node {
    async fn start(home: &Home) -> Self {
        let port = StdListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let args = RunArgs {
            port: Some(port.to_string()),
            local_source: Some("off".to_owned()),
            ..RunArgs::default()
        };
        let location = LocationArgs {
            home: Some(home.0.clone()),
            service: false,
        };
        let (stop, stopped) = oneshot::channel::<()>();
        let done = tokio::spawn(async move {
            folo_node::node::run_with(&location, &args, None, async {
                let _ = stopped.await;
            })
            .await
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "the node never listened");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Self {
            port,
            stop: Some(stop),
            done,
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> Answer {
        let (port, method, path, content_type) = (
            self.port,
            method.to_owned(),
            path.to_owned(),
            content_type.map(str::to_owned),
        );
        tokio::task::spawn_blocking(move || {
            exchange(port, &method, &path, content_type.as_deref(), &body)
        })
        .await
        .unwrap()
    }

    async fn get(&self, path: &str) -> Answer {
        self.call("GET", path, None, Vec::new()).await
    }

    async fn install(&self, entries: &Entries, query: &str) -> Answer {
        let bytes = archive::write(entries).unwrap();
        self.call(
            "POST",
            &format!("/api/v1/library/packs{query}"),
            Some("application/zip"),
            bytes,
        )
        .await
    }

    async fn put(&self, path: &str, body: &Value) -> Answer {
        self.call(
            "PUT",
            path,
            Some("application/json"),
            body.to_string().into_bytes(),
        )
        .await
    }

    async fn stop(mut self) -> u8 {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(30), self.done)
            .await
            .expect("the node stops")
            .unwrap()
    }
}

fn pack(id: &str, version: &str, extra: &Value, items: &[(&str, &str, Value)]) -> Entries {
    let mut files = Entries::new();
    let mut contents = serde_json::Map::new();
    for (folder, stem, item) in items {
        files.insert(
            format!("{folder}/{stem}.json"),
            serde_json::to_vec(item).unwrap(),
        );
        let key = if *folder == "zone-templates" {
            "zone_templates"
        } else {
            folder
        };
        contents
            .entry(key)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(item["id"].clone());
    }
    let hashes: serde_json::Map<String, Value> = files
        .iter()
        .map(|(p, b)| (p.clone(), json!(hex(&Sha256::digest(b)))))
        .collect();
    let mut manifest = json!({
        "id": id, "version": version, "folo": ">=1.0 <2", "name": "Test", "description": "d",
        "license": "MIT", "authors": ["tester"], "contents": contents, "files": hashes,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        manifest[key] = value;
    }
    files.insert(
        "folo-pack.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    files
}

fn palette(id: &str) -> Value {
    json!({"id": id, "schema_version": 2, "name": id, "colors": ["#0033CC"]})
}

fn scene(id: &str) -> Value {
    json!({"id": id, "schema_version": 2, "scene": {"id": id, "name": id}})
}

fn conforms<T: serde::de::DeserializeOwned>(value: &Value, what: &str) {
    if let Err(error) = serde_json::from_value::<T>(value.clone()) {
        panic!("{what} doesn't conform: {error}\n{value}");
    }
}

fn simple(id: &str, version: &str) -> Entries {
    pack(
        id,
        version,
        &json!({}),
        &[
            ("palettes", "p1", palette("one")),
            ("scenes", "s1", scene("two")),
        ],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_library_routes() {
    let home = Home::new();
    let node = Node::start(&home).await;
    reading_the_built_ins(&node).await;
    installing(&node).await;
    refusals(&node).await;
    the_zip_routes_and_their_limits(&node).await;
    removing(&node).await;
    local_items(&node).await;
    events(&node).await;
    assert_eq!(node.stop().await, 0);
    // The library is on disk, so a restart finds it
    let again = Node::start(&home).await;
    let packs = again.get("/api/v1/library/packs").await.json();
    let ids: Vec<&str> = packs
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"acme/lights") && ids.contains(&"local/my-presets"),
        "{ids:?}"
    );
    assert_eq!(again.stop().await, 0);
}

async fn reading_the_built_ins(node: &Node) {
    let list = node.get("/api/v1/library/packs").await;
    assert_eq!(list.status, 200);
    let packs = list.json();
    for pack in packs.as_array().unwrap() {
        conforms::<PackInfo>(pack, "a PackInfo");
    }
    let ids: Vec<&str> = packs
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "folo/core",
            "folo/effects",
            "folo/genres",
            "local/my-presets"
        ]
    );

    // A pack id is percent-encoded in a path
    let manifest = node.get("/api/v1/library/packs/folo%2Fcore").await;
    assert_eq!(manifest.status, 200);
    conforms::<PackManifest>(&manifest.json(), "a manifest");
    assert_eq!(manifest.json()["id"], "folo/core");
    assert_eq!(
        node.get("/api/v1/library/packs/folo%2Fnone").await.code(),
        "not_found"
    );
    assert_eq!(node.get("/api/v1/library/packs/nothing").await.status, 404);

    let item = node
        .get("/api/v1/library/packs/folo%2Fgenres/items/scenes/edm")
        .await;
    assert_eq!(item.status, 200);
    assert_eq!(item.json()["scene"]["name"], "EDM festival");
    assert_eq!(
        node.get("/api/v1/library/packs/folo%2Fgenres/items/scenes/none")
            .await
            .code(),
        "not_found"
    );
    assert_eq!(
        node.get("/api/v1/library/packs/folo%2Fgenres/items/widgets/edm")
            .await
            .code(),
        "not_found"
    );
    assert_eq!(
        node.get("/api/v1/library/packs/folo%2Fcore/items/palettes/deep-night")
            .await
            .status,
        200
    );

    let scenes = node.get("/api/v1/library/items").await.json();
    assert_eq!(
        scenes.as_array().unwrap().len(),
        1 + 14 + 32,
        "scenes is the default type"
    );
    conforms::<LibraryItem>(&scenes[0], "a LibraryItem");
    assert_eq!(scenes[0]["ref"], "folo/core:default");
    let palettes = node.get("/api/v1/library/items?type=palettes").await.json();
    assert_eq!(palettes.as_array().unwrap().len(), 9);
    assert_eq!(
        node.get("/api/v1/library/items?type=zone-templates")
            .await
            .json(),
        json!([])
    );
    assert_eq!(
        node.get("/api/v1/library/items?type=bogus").await.code(),
        "bad_query"
    );

    let exported = node.get("/api/v1/library/packs/folo%2Fcore/export").await;
    assert_eq!(exported.status, 200);
    assert_eq!(exported.header("content-type"), Some("application/zip"));
    assert!(
        exported
            .header("content-disposition")
            .unwrap()
            .contains("folo-core.folopack")
    );
    let hash = hex(&Sha256::digest(&exported.body));
    let info = packs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "folo/core")
        .unwrap();
    assert_eq!(
        info["hash"], hash,
        "the pack hash is the hash of the export"
    );
    assert_eq!(
        node.get("/api/v1/library/packs/folo%2Fnone/export")
            .await
            .status,
        404
    );
}

async fn installing(node: &Node) {
    let installed = node.install(&simple("acme/lights", "1.0.0"), "").await;
    assert_eq!(
        installed.status,
        201,
        "{}",
        String::from_utf8_lossy(&installed.body)
    );
    conforms::<PackInfo>(&installed.json(), "the installed PackInfo");
    assert_eq!(installed.json()["items"], 2);
    let item = node
        .get("/api/v1/library/packs/acme%2Flights/items/palettes/one")
        .await;
    assert_eq!(item.json()["name"], "one");
    let refs = node.get("/api/v1/library/items?type=scenes").await.json();
    assert!(
        refs.as_array()
            .unwrap()
            .iter()
            .any(|i| i["ref"] == "acme/lights:two")
    );

    // Export, remove, install again: the same pack
    let before = node.get("/api/v1/library/packs").await.json();
    let hash = before
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "acme/lights")
        .unwrap()["hash"]
        .clone();
    let exported = node.get("/api/v1/library/packs/acme%2Flights/export").await;
    assert_eq!(
        node.call(
            "DELETE",
            "/api/v1/library/packs/acme%2Flights",
            Some("application/json"),
            Vec::new()
        )
        .await
        .status,
        204
    );
    let again = node
        .call(
            "POST",
            "/api/v1/library/packs",
            Some("application/zip; charset=binary"),
            exported.body.clone(),
        )
        .await;
    assert_eq!(again.status, 201);
    assert_eq!(again.json()["hash"], hash);

    // A newer version replaces; the same or an older one needs replace=true
    assert_eq!(
        node.install(&simple("acme/lights", "1.1.0"), "")
            .await
            .status,
        201
    );
    let older = node.install(&simple("acme/lights", "1.0.5"), "").await;
    assert_eq!(
        (older.status, older.code()),
        (409, "already_installed".to_owned())
    );
    assert_eq!(
        older.details(),
        json!({"installed": "1.1.0", "offered": "1.0.5"})
    );
    let forced = node
        .install(&simple("acme/lights", "1.0.5"), "?replace=true")
        .await;
    assert_eq!(
        (forced.status, forced.json()["version"].clone()),
        (201, json!("1.0.5"))
    );
    assert_eq!(
        node.install(&simple("acme/lights", "1.0.5"), "?replace=false")
            .await
            .status,
        409
    );
    assert_eq!(
        node.install(&simple("acme/lights", "1.0.5"), "?replace=maybe")
            .await
            .code(),
        "bad_query"
    );
}

async fn refusals(node: &Node) {
    for id in ["folo/extra", "local/shared"] {
        let reserved = node.install(&simple(id, "1.0.0"), "").await;
        assert_eq!(
            (reserved.status, reserved.code()),
            (409, "reserved_id".to_owned()),
            "{id}"
        );
    }
    let mut broken = simple("acme/broken", "1.0.0");
    broken.insert("scenes/stray.json".into(), b"{}".to_vec());
    let structural = node.install(&broken, "").await;
    assert_eq!(
        (structural.status, structural.code()),
        (422, "structural_error".to_owned())
    );
    assert_eq!(structural.details()["path"], "scenes/stray.json");
    assert!(
        structural.details()["reason"]
            .as_str()
            .unwrap()
            .contains("listed in files")
    );
    assert_eq!(
        node.get("/api/v1/library/packs/acme%2Fbroken").await.status,
        404
    );

    let garbage = node
        .call(
            "POST",
            "/api/v1/library/packs",
            Some("application/zip"),
            b"not a zip".to_vec(),
        )
        .await;
    assert_eq!(
        (garbage.status, garbage.code()),
        (422, "structural_error".to_owned())
    );

    let needs = pack(
        "acme/needs",
        "1.0.0",
        &json!({"requires": {"acme/absent": "^1"}}),
        &[],
    );
    let missing = node.install(&needs, "").await;
    assert_eq!(
        (missing.status, missing.code()),
        (422, "missing_dependency".to_owned())
    );
    assert_eq!(
        missing.details(),
        json!({"requires": {"acme/absent": "^1"}})
    );

    // A development build (version 0.0.0) doesn't check folo ranges, so
    // `incompatible` is covered by the library's tests
    let future = pack("acme/future", "1.0.0", &json!({"folo": ">=9"}), &[]);
    assert_eq!(node.install(&future, "").await.status, 201);
    node.call(
        "DELETE",
        "/api/v1/library/packs/acme%2Ffuture",
        Some("application/json"),
        Vec::new(),
    )
    .await;
}

async fn the_zip_routes_and_their_limits(node: &Node) {
    let entries = simple("acme/zip", "1.0.0");
    let bytes = archive::write(&entries).unwrap();
    // The install takes a zip, not JSON; every other mutating request keeps the JSON rule
    let as_json = node
        .call(
            "POST",
            "/api/v1/library/packs",
            Some("application/json"),
            bytes.clone(),
        )
        .await;
    assert_eq!(
        (as_json.status, as_json.code()),
        (415, "zip_required".to_owned())
    );
    let none = node
        .call("POST", "/api/v1/library/packs", None, bytes.clone())
        .await;
    assert_eq!(none.status, 415);
    let as_text = node
        .call(
            "PUT",
            "/api/v1/library/packs/local%2Fmy-presets/items/scenes/x",
            Some("application/zip"),
            b"{}".to_vec(),
        )
        .await;
    assert_eq!(
        (as_text.status, as_text.code()),
        (415, "json_required".to_owned())
    );
    let delete = node
        .call(
            "DELETE",
            "/api/v1/library/packs/acme%2Fzip",
            Some("application/zip"),
            Vec::new(),
        )
        .await;
    assert_eq!(delete.code(), "json_required");

    // 10 MB for an install, 1 MB for everything else
    let mut big = simple("acme/big", "1.0.0");
    let mut preview = b"\x89PNG\r\n\x1a\n".to_vec();
    // Noise, so deflate can't shrink it: xorshift
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    preview.extend((0..1_900_000).map(|_| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 24) as u8
    }));
    for n in 0..4 {
        big.insert(format!("previews/p{n}.png"), preview.clone());
    }
    let mut manifest: Value = serde_json::from_slice(&big["folo-pack.json"]).unwrap();
    manifest["files"] = {
        let hashes: serde_json::Map<String, Value> = big
            .iter()
            .filter(|(p, _)| p.as_str() != "folo-pack.json")
            .map(|(p, b)| (p.clone(), json!(hex(&Sha256::digest(b)))))
            .collect();
        Value::Object(hashes)
    };
    big.insert(
        "folo-pack.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    let body = archive::write(&big).unwrap();
    assert!(
        body.len() > 1024 * 1024 && body.len() < 10 * 1024 * 1024,
        "{}",
        body.len()
    );
    assert_eq!(
        node.call(
            "POST",
            "/api/v1/library/packs",
            Some("application/zip"),
            body
        )
        .await
        .status,
        201
    );
    let shown = node
        .get("/api/v1/library/packs/acme%2Fbig/previews/p2.png")
        .await;
    assert_eq!(shown.status, 200);
    assert_eq!(shown.body, preview);
    assert_eq!(shown.header("content-type"), Some("image/png"));
    assert_eq!(
        shown.header("content-security-policy"),
        Some("default-src 'none'; sandbox")
    );
    assert_eq!(
        node.get("/api/v1/library/packs/acme%2Fbig/previews/none.png")
            .await
            .code(),
        "not_found"
    );
    assert_eq!(
        node.get("/api/v1/library/packs/acme%2Fnone/previews/p2.png")
            .await
            .status,
        404
    );
    let too_big = vec![0u8; 10 * 1024 * 1024 + 1];
    let refused = node
        .call(
            "POST",
            "/api/v1/library/packs",
            Some("application/zip"),
            too_big,
        )
        .await;
    assert_eq!(
        (refused.status, refused.code()),
        (413, "body_too_large".to_owned())
    );
    let big_json = json!({"id": "x", "pad": "x".repeat(1024 * 1024)});
    let refused = node
        .put(
            "/api/v1/library/packs/local%2Fmy-presets/items/scenes/x",
            &big_json,
        )
        .await;
    assert_eq!(
        (refused.status, refused.code()),
        (413, "body_too_large".to_owned())
    );
    node.call(
        "DELETE",
        "/api/v1/library/packs/acme%2Fbig",
        Some("application/json"),
        Vec::new(),
    )
    .await;
}

async fn removing(node: &Node) {
    let base = simple("acme/base", "1.0.0");
    assert_eq!(node.install(&base, "").await.status, 201);
    let user = pack(
        "acme/user",
        "1.0.0",
        &json!({"requires": {"acme/base": "^1"}}),
        &[],
    );
    assert_eq!(node.install(&user, "").await.status, 201);
    let delete = |id: &'static str| async move {
        node.call(
            "DELETE",
            &format!("/api/v1/library/packs/{id}"),
            Some("application/json"),
            Vec::new(),
        )
        .await
    };
    let required = delete("acme%2Fbase").await;
    assert_eq!(
        (required.status, required.code()),
        (409, "required_by".to_owned())
    );
    assert_eq!(required.details(), json!({"packs": ["acme/user"]}));
    for protected in ["folo%2Fcore", "local%2Fmy-presets"] {
        let answer = delete(protected).await;
        assert_eq!(
            (answer.status, answer.code()),
            (409, "protected_pack".to_owned())
        );
    }
    assert_eq!(delete("acme%2Fuser").await.status, 204);
    assert_eq!(delete("acme%2Fbase").await.status, 204);
    assert_eq!(delete("acme%2Fbase").await.status, 404);
}

async fn local_items(node: &Node) {
    let base = "/api/v1/library/packs/local%2Fmy-presets/items";
    let stored = node
        .put(
            &format!("{base}/scenes/mine"),
            &json!({"scene": {"name": "Mine"}, "extra": 1}),
        )
        .await;
    assert_eq!(stored.status, 200);
    assert_eq!(stored.json()["id"], "mine");
    assert!(stored.json().get("extra").is_none());
    assert_eq!(
        node.get(&format!("{base}/scenes/mine")).await.json()["scene"]["name"],
        "Mine"
    );
    let listed = node.get("/api/v1/library/items").await.json();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["ref"] == "local/my-presets:mine")
    );

    let taken = node
        .put(&format!("{base}/palettes/mine"), &palette("mine"))
        .await;
    assert_eq!((taken.status, taken.code()), (409, "id_taken".to_owned()));
    let bad = node.put(&format!("{base}/scenes/x"), &json!([1])).await;
    assert_eq!((bad.status, bad.code()), (400, "bad_body".to_owned()));
    let not_json = node
        .call(
            "PUT",
            &format!("{base}/scenes/x"),
            Some("application/json"),
            b"nope".to_vec(),
        )
        .await;
    assert_eq!(
        (not_json.status, not_json.code()),
        (400, "bad_body".to_owned())
    );
    let newer = node
        .put(&format!("{base}/scenes/x"), &json!({"schema_version": 3}))
        .await;
    assert_eq!(
        (newer.status, newer.code()),
        (422, "structural_error".to_owned())
    );
    let kind = node.put(&format!("{base}/widgets/x"), &scene("x")).await;
    assert_eq!(kind.status, 404);
    let none = node
        .put(
            "/api/v1/library/packs/local%2Fnone/items/scenes/x",
            &scene("x"),
        )
        .await;
    assert_eq!(none.status, 404);

    let read_only = node
        .put(
            "/api/v1/library/packs/folo%2Fgenres/items/scenes/edm",
            &scene("edm"),
        )
        .await;
    assert_eq!(
        (read_only.status, read_only.code()),
        (409, "read_only_pack".to_owned())
    );
    let delete_ro = node
        .call(
            "DELETE",
            "/api/v1/library/packs/folo%2Fgenres/items/scenes/edm",
            Some("application/json"),
            Vec::new(),
        )
        .await;
    assert_eq!(delete_ro.code(), "read_only_pack");

    for n in 0..59 {
        assert_eq!(
            node.put(&format!("{base}/scenes/s{n}"), &scene("x"))
                .await
                .status,
            200
        );
    }
    let full = node
        .put(&format!("{base}/scenes/one-more"), &scene("x"))
        .await;
    assert_eq!((full.status, full.code()), (409, "list_full".to_owned()));
    let delete = node
        .call(
            "DELETE",
            &format!("{base}/scenes/mine"),
            Some("application/json"),
            Vec::new(),
        )
        .await;
    assert_eq!(delete.status, 204);
    assert_eq!(node.get(&format!("{base}/scenes/mine")).await.status, 404);
    let gone = node
        .call(
            "DELETE",
            &format!("{base}/scenes/mine"),
            Some("application/json"),
            Vec::new(),
        )
        .await;
    assert_eq!(gone.status, 404);
}

async fn events(node: &Node) {
    let url = format!("ws://127.0.0.1:{}/api/v1/live", node.port);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Wait until the connection is served before changing the library
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        node.install(&simple("acme/events", "1.0.0"), "")
            .await
            .status,
        201
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no library event");
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["type"] == "event" {
                conforms::<LiveEventMessage>(&message, "an event message");
                assert_eq!(
                    message["data"],
                    json!({"kind": "library", "id": "acme/events", "state": "installed"})
                );
                break;
            }
        }
    }
    node.call(
        "DELETE",
        "/api/v1/library/packs/acme%2Fevents",
        Some("application/json"),
        Vec::new(),
    )
    .await;
}
