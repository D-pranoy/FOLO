//! Backup and restore end to end, in this process (RB-09 change 5):
//! docs/specs/client-api.md#backup-and-restore, every rejection of a
//! restore, the exclusions, and an interrupted restore finished at start.

#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::format_push_string,
    clippy::cast_possible_truncation
)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folo_library::archive::{self, ArchiveLimits, Entries};
use folo_library::backup::MANIFEST;
use folo_library::canonical::hex;
use folo_node::cli::{LocationArgs, RunArgs};
use folo_protocol::generated::BackupManifest;
use futures_util::StreamExt as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-backup-api-{}", folo_store::new_id()));
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

const ZIP: &str = "application/zip";

fn data_dir(home: &Home) -> PathBuf {
    find_file(&home.0, "secrets.json")
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .expect("the node wrote its secrets")
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

fn pack_entries(id: &str, version: &str) -> Entries {
    let item = json!({"id": "one", "schema_version": 2, "name": "one", "colors": ["#0033CC"]});
    let mut files = Entries::new();
    files.insert(
        "palettes/p1.json".into(),
        serde_json::to_vec(&item).unwrap(),
    );
    let hashes: serde_json::Map<String, Value> = files
        .iter()
        .map(|(p, b)| (p.clone(), json!(hex(&Sha256::digest(b)))))
        .collect();
    let manifest = json!({
        "id": id, "version": version, "folo": ">=1.0 <2", "name": "Test", "description": "d",
        "license": "MIT", "authors": ["tester"], "contents": {"palettes": ["one"]}, "files": hashes,
    });
    files.insert(
        "folo-pack.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    files
}

fn surface(id: &str) -> Value {
    json!({"id": id, "kind": "strip", "role": "strip.main", "resolution": {"width": 30, "height": 1},
        "master": 0.4, "background": "#112233"})
}

impl Node {
    async fn send(&self, method: &str, path: &str, body: &Value) -> Answer {
        self.call(
            method,
            path,
            Some("application/json"),
            body.to_string().into_bytes(),
        )
        .await
    }

    async fn restore(&self, bytes: Vec<u8>) -> Answer {
        self.call("POST", "/api/v1/system/restore", Some(ZIP), bytes)
            .await
    }

    async fn document(&self, name: &str) -> Value {
        self.get(&format!("/api/v1/{name}")).await.json()
    }
}

/// A node holding a zone, a pack, a local preset and an applied scene.
async fn furnished(home: &Home) -> Node {
    let node = Node::start(home).await;
    let zone = json!({"id": "z1", "name": "Desk", "surfaces": [surface("s1")]});
    assert_eq!(node.send("POST", "/api/v1/zones", &zone).await.status, 201);
    assert_eq!(
        node.install(&pack_entries("acme/lights", "1.2.0"), "")
            .await
            .status,
        201
    );
    let applied = node
        .send(
            "POST",
            "/api/v1/scenes/genre%3Aedm/apply",
            &json!({"zone": "z1"}),
        )
        .await;
    assert_eq!(
        applied.status,
        200,
        "{}",
        String::from_utf8_lossy(&applied.body)
    );
    let preset = json!({"id": "mine", "schema_version": 2, "name": "mine", "colors": ["#FF3399"]});
    let put = node
        .put(
            "/api/v1/library/packs/local%2Fmy-presets/items/palettes/mine",
            &preset,
        )
        .await;
    assert_eq!(put.status, 200, "{}", String::from_utf8_lossy(&put.body));
    node
}

/// A backup's entries, read without limits worth mentioning.
fn entries_of(bytes: &[u8]) -> Entries {
    let limit = ArchiveLimits {
        entries: 100_000,
        uncompressed: 1 << 30,
    };
    archive::read_with(bytes, limit, &|_| Ok(1 << 30)).unwrap()
}

/// `entries` as a backup whose manifest lists them as they are.
fn rewrap(mut entries: Entries) -> Vec<u8> {
    entries.remove(MANIFEST);
    let hashes: serde_json::Map<String, Value> = entries
        .iter()
        .map(|(p, b)| (p.clone(), json!(hex(&Sha256::digest(b)))))
        .collect();
    let manifest = json!({"format": 1, "created": 1, "node_version": "0.0.0",
        "schema_version": 2, "files": hashes});
    entries.insert(MANIFEST.into(), serde_json::to_vec(&manifest).unwrap());
    archive::write_first(&entries, MANIFEST).unwrap()
}

/// The backup with `change` applied to its entries and its manifest recomputed.
fn tampered(good: &[u8], change: impl FnOnce(&mut Entries)) -> Vec<u8> {
    let mut entries = entries_of(good);
    change(&mut entries);
    rewrap(entries)
}

/// The backup with `change` applied to its manifest, the files untouched.
fn with_manifest(good: &[u8], change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut entries = entries_of(good);
    let mut manifest: Value = serde_json::from_slice(&entries[MANIFEST]).unwrap();
    change(&mut manifest);
    entries.insert(MANIFEST.into(), serde_json::to_vec(&manifest).unwrap());
    archive::write_first(&entries, MANIFEST).unwrap()
}

fn base_names(entries: &Entries) -> Vec<&String> {
    entries
        .keys()
        .filter(|p| p.starts_with("applied/"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_restores_on_another_node() {
    let (home_a, home_b) = (Home::new(), Home::new());
    let a = furnished(&home_a).await;
    let saved = a.get("/api/v1/system/backup").await;
    assert_eq!(saved.status, 200);
    assert_eq!(saved.header("content-type"), Some(ZIP));
    let disposition = saved.header("content-disposition").unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"folo-backup-")
            && disposition.ends_with("Z.zip\""),
        "{disposition}"
    );
    let entries = entries_of(&saved.body);
    let mut names: Vec<&str> = entries.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert!(names.contains(&"folo-backup.json"));
    for document in ["zones.json", "show.json", "providers.json", "sources.json"] {
        assert!(names.contains(&document), "{names:?}");
    }
    assert!(
        names.contains(&"library/acme/lights/folo-pack.json"),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("library/local/my-presets/palettes/")),
        "{names:?}"
    );
    assert_eq!(base_names(&entries).len(), 1, "{names:?}");
    // Keys, client keys, host settings and the built-in packs stay out
    for name in &names {
        assert!(
            !["secrets.json", "clients.json", "node.toml"].contains(name)
                && !name.starts_with("library/folo/")
                && !name.starts_with("restore"),
            "{name}"
        );
    }
    let manifest: Value = serde_json::from_slice(&entries[MANIFEST]).unwrap();
    serde_json::from_value::<BackupManifest>(manifest.clone()).unwrap();
    assert_eq!(manifest["format"], 1);
    assert_eq!(manifest["schema_version"], 2);
    let (zones_a, show_a) = (a.document("zones").await, a.document("show").await);
    assert_eq!(a.stop().await, 0);

    // Another node, with a pack the backup doesn't have
    let b = Node::start(&home_b).await;
    assert_eq!(
        b.install(&pack_entries("acme/old", "1.0.0"), "")
            .await
            .status,
        201
    );
    let secrets_before = std::fs::read(data_dir(&home_b).join("secrets.json")).unwrap();
    let (zones_b, show_b) = (b.document("zones").await, b.document("show").await);
    let url = format!("ws://127.0.0.1:{}/api/v1/live", b.port);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let restored = b.restore(saved.body.clone()).await;
    assert_eq!(
        restored.status,
        204,
        "{}",
        String::from_utf8_lossy(&restored.body)
    );
    let (zones_now, show_now) = (b.document("zones").await, b.document("show").await);
    for (now, was_b, was_a) in [
        (&zones_now, &zones_b, &zones_a),
        (&show_now, &show_b, &show_a),
    ] {
        assert_eq!(
            now["revision"].as_u64().unwrap(),
            was_b["revision"].as_u64().unwrap() + 1
        );
        assert_eq!(now["server_id"], was_b["server_id"]);
        let (mut left, mut right) = (now.clone(), was_a.clone());
        for value in [&mut left, &mut right] {
            value.as_object_mut().unwrap().remove("revision");
            value.as_object_mut().unwrap().remove("server_id");
        }
        assert_eq!(left, right, "the document is the backup's");
    }
    let packs = b.get("/api/v1/library/packs").await.json();
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
    assert!(
        !ids.contains(&"acme/old"),
        "the library is exactly the backup's: {ids:?}"
    );
    let items = b
        .get("/api/v1/library/items?type=palettes")
        .await
        .json()
        .to_string();
    assert!(items.contains("local/my-presets:mine"), "{items}");
    // The bases came too, and the keys stayed
    let data = data_dir(&home_b);
    let bases: Vec<_> = std::fs::read_dir(data.join("applied"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(bases.len(), 1);
    assert_eq!(
        std::fs::read(data.join("secrets.json")).unwrap(),
        secrets_before
    );
    assert!(!data.join("restore").exists() && !data.join("restore.pending").exists());
    // The live channel told clients
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.len() < 5 {
        assert!(Instant::now() < deadline, "no restore events: {seen:?}");
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["type"] == "config_changed" {
                seen.push(message["data"]["document"].as_str().unwrap().to_owned());
            } else if message["type"] == "event" {
                assert_eq!(
                    message["data"],
                    json!({"kind": "library", "id": "", "state": "restored"})
                );
                seen.push("event".into());
            }
        }
    }
    for expected in ["zones", "show", "providers", "sources", "event"] {
        assert!(seen.iter().any(|s| s == expected), "{expected} in {seen:?}");
    }
    assert_eq!(b.stop().await, 0);
    // It is on disk: a restart finds it
    let again = Node::start(&home_b).await;
    assert_eq!(again.document("zones").await["zones"], zones_a["zones"]);
    let packs = again.get("/api/v1/library/packs").await.json();
    assert!(packs.to_string().contains("acme/lights"));
    assert_eq!(again.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_checks_everything_before_changing_anything() {
    let (home_a, home_b) = (Home::new(), Home::new());
    let a = furnished(&home_a).await;
    let good = a.get("/api/v1/system/backup").await.body;
    assert_eq!(a.stop().await, 0);
    let b = Node::start(&home_b).await;
    assert_eq!(
        b.install(&pack_entries("acme/old", "1.0.0"), "")
            .await
            .status,
        201
    );
    let before = (b.document("zones").await, b.document("show").await);
    let packs_before = b.get("/api/v1/library/packs").await.body;

    let base = entries_of(&good)
        .keys()
        .find(|p| p.starts_with("applied/"))
        .unwrap()
        .clone();
    let document = |entries: &mut Entries, name: &str, value: &Value| {
        entries.insert(name.into(), serde_json::to_vec(value).unwrap());
    };
    let cases: Vec<(&str, Vec<u8>, u16, &str)> = vec![
        (
            "not a zip",
            b"just some bytes".to_vec(),
            422,
            "structural_error",
        ),
        (
            "a path leaving the folder",
            tampered(&good, |e| {
                e.insert("../evil.json".into(), b"{}".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "a pack path leaving the folder",
            tampered(&good, |e| {
                e.insert(
                    "library/acme/lights/../../../evil.json".into(),
                    b"{}".to_vec(),
                );
            }),
            422,
            "structural_error",
        ),
        (
            "an absolute path",
            tampered(&good, |e| {
                e.insert("/etc/passwd".into(), b"x".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "a backslash path",
            tampered(&good, |e| {
                e.insert("library\\acme\\x.json".into(), b"{}".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "the secrets",
            tampered(&good, |e| {
                e.insert("secrets.json".into(), b"{}".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "the client keys",
            tampered(&good, |e| {
                e.insert("clients.json".into(), b"{}".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "the host settings",
            tampered(&good, |e| {
                e.insert("node.toml".into(), b"x".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "a restore folder",
            tampered(&good, |e| {
                e.insert("restore/zones.json".into(), b"{}".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "a built-in pack",
            tampered(&good, |e| {
                for (p, b) in entries_of(&good) {
                    if let Some(rest) = p.strip_prefix("library/acme/lights/") {
                        e.insert(format!("library/folo/core/{rest}"), b);
                    }
                }
            }),
            422,
            "structural_error",
        ),
        (
            "a base that isn't named by a hash",
            tampered(&good, |e| {
                e.insert("applied/notahash.json".into(), b"{}".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "an upper case base name",
            tampered(&good, |e| {
                let bytes = e.remove(&base).unwrap();
                e.insert(
                    base.to_uppercase()
                        .replace("APPLIED/", "applied/")
                        .replace(".JSON", ".json"),
                    bytes,
                );
            }),
            422,
            "structural_error",
        ),
        (
            "a pack entry too deep",
            tampered(&good, |e| {
                e.insert(
                    "library/acme/lights/palettes/deep/p.json".into(),
                    b"{}".to_vec(),
                );
            }),
            422,
            "structural_error",
        ),
        (
            "a document missing",
            tampered(&good, |e| {
                e.remove("providers.json");
            }),
            422,
            "structural_error",
        ),
        (
            "a document that isn't JSON",
            tampered(&good, |e| {
                e.insert("show.json".into(), b"not json".to_vec());
            }),
            422,
            "structural_error",
        ),
        (
            "a document that isn't an object",
            tampered(&good, |e| {
                document(e, "zones.json", &json!([1]));
            }),
            422,
            "structural_error",
        ),
        (
            "a document from a newer FOLO",
            tampered(&good, |e| {
                document(e, "zones.json", &json!({"schema_version": 3}));
            }),
            422,
            "incompatible",
        ),
        (
            "a folder that isn't its pack's id",
            tampered(&good, |e| {
                for (p, b) in entries_of(&good) {
                    if let Some(rest) = p.strip_prefix("library/acme/lights/") {
                        e.remove(&p);
                        e.insert(format!("library/acme/other/{rest}"), b);
                    }
                }
            }),
            422,
            "structural_error",
        ),
        (
            "a pack without its manifest",
            tampered(&good, |e| {
                e.remove("library/acme/lights/folo-pack.json");
            }),
            422,
            "structural_error",
        ),
        (
            "a bad pack",
            tampered(&good, |e| {
                e.insert(
                    "library/acme/lights/palettes/p1.json".into(),
                    b"not json".to_vec(),
                );
            }),
            422,
            "structural_error",
        ),
        (
            "a file the manifest doesn't list",
            {
                let mut entries = entries_of(&good);
                entries.insert(
                    "applied/".to_owned() + &"0".repeat(64) + ".json",
                    b"{}".to_vec(),
                );
                archive::write_first(&entries, MANIFEST).unwrap()
            },
            422,
            "structural_error",
        ),
        (
            "a hash that doesn't match",
            {
                let mut entries = entries_of(&good);
                entries.insert("sources.json".into(), b"{}".to_vec());
                archive::write_first(&entries, MANIFEST).unwrap()
            },
            422,
            "structural_error",
        ),
        (
            "a listed file that isn't there",
            with_manifest(&good, |m| {
                m["files"]["applied/".to_owned() + &"1".repeat(64) + ".json"] =
                    json!("0".repeat(64));
            }),
            422,
            "structural_error",
        ),
        (
            "a manifest with a field of its own",
            with_manifest(&good, |m| {
                m["extra"] = json!(1);
            }),
            422,
            "structural_error",
        ),
        (
            "a format that isn't a number",
            with_manifest(&good, |m| {
                m["format"] = json!("1");
            }),
            422,
            "structural_error",
        ),
        (
            "a newer format",
            with_manifest(&good, |m| {
                m["format"] = json!(2);
            }),
            422,
            "incompatible",
        ),
        (
            "a newer schema version",
            with_manifest(&good, |m| {
                m["schema_version"] = json!(3);
            }),
            422,
            "incompatible",
        ),
        (
            "no manifest",
            {
                let mut entries = entries_of(&good);
                entries.remove(MANIFEST);
                archive::write_first(&entries, "zones.json").unwrap()
            },
            422,
            "structural_error",
        ),
    ];
    for (what, bytes, status, code) in cases {
        let answer = b.restore(bytes).await;
        assert_eq!(
            (answer.status, answer.code().as_str()),
            (status, code),
            "{what}: {}",
            String::from_utf8_lossy(&answer.body)
        );
        let (zones, show) = (b.document("zones").await, b.document("show").await);
        assert_eq!(
            (&zones, &show),
            (&before.0, &before.1),
            "{what} changed a document"
        );
        assert_eq!(
            b.get("/api/v1/library/packs").await.body,
            packs_before,
            "{what} changed the library"
        );
        assert!(
            !data_dir(&home_b).join("restore.pending").exists(),
            "{what}"
        );
    }
    // Not a zip at all: the route's own rule
    let json = b
        .call(
            "POST",
            "/api/v1/system/restore",
            Some("application/json"),
            good.clone(),
        )
        .await;
    assert_eq!((json.status, json.code().as_str()), (415, "zip_required"));
    let none = b
        .call("POST", "/api/v1/system/restore", None, good.clone())
        .await;
    assert_eq!(none.status, 415);
    // The same backup, untouched, restores
    assert_eq!(b.restore(good).await.status, 204);
    assert_eq!(b.stop().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_refuses_bodies_over_its_limit() {
    let home = Home::new();
    let node = Node::start(&home).await;
    // 256 MB is the limit, and the zeros of a bomb compress to almost nothing:
    // what counts is the decompressed size
    let mut entries = Entries::new();
    entries.insert("zones.json".into(), vec![b' '; 40 * 1024 * 1024]);
    let bomb = rewrap(entries);
    assert!(bomb.len() < 1024 * 1024, "{}", bomb.len());
    let answer = node.restore(bomb).await;
    assert_eq!(
        (answer.status, answer.code().as_str()),
        (422, "structural_error")
    );
    assert!(
        answer.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("zones.json")
    );
    assert_eq!(node.stop().await, 0);
}

fn stage_from(good: &[u8], data: &Path, skip: &[&str]) {
    let staging = data.join("restore");
    for (path, bytes) in entries_of(good) {
        if path == MANIFEST || skip.contains(&path.as_str()) {
            continue;
        }
        let target = staging.join(&path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        if path.starts_with("library/") {
            // Packs are staged in the library's form: the archive's canonical bytes are valid too
            std::fs::write(&target, &bytes).unwrap();
        } else {
            std::fs::write(&target, &bytes).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_restore_is_finished_at_start() {
    let (home_a, home_b) = (Home::new(), Home::new());
    let a = furnished(&home_a).await;
    let good = a.get("/api/v1/system/backup").await.body;
    assert_eq!(a.stop().await, 0);
    let b = Node::start(&home_b).await;
    assert_eq!(
        b.install(&pack_entries("acme/old", "1.0.0"), "")
            .await
            .status,
        201
    );
    assert_eq!(b.stop().await, 0);
    let data = data_dir(&home_b);

    // A restore/ without its marker never completed: it is thrown away
    std::fs::create_dir_all(data.join("restore")).unwrap();
    std::fs::write(data.join("restore/zones.json"), b"{\"zones\": []}").unwrap();
    let node = Node::start(&home_b).await;
    assert!(!data.join("restore").exists());
    assert!(
        node.get("/api/v1/library/packs")
            .await
            .json()
            .to_string()
            .contains("acme/old")
    );
    assert_eq!(node.stop().await, 0);

    // With the marker, and some moves already made (zones.json is in place,
    // the rest is waiting): the node finishes them before it reads anything
    stage_from(&good, &data, &["zones.json"]);
    std::fs::write(
        data.join("zones.json"),
        entries_of(&good)["zones.json"].clone(),
    )
    .unwrap();
    std::fs::write(data.join("restore.pending"), b"restore/ is complete\n").unwrap();
    let node = Node::start(&home_b).await;
    assert!(!data.join("restore").exists() && !data.join("restore.pending").exists());
    let packs = node.get("/api/v1/library/packs").await.json().to_string();
    assert!(
        packs.contains("acme/lights") && !packs.contains("acme/old"),
        "{packs}"
    );
    let zones = node.document("zones").await;
    assert_eq!(zones["zones"][0]["id"], "z1");
    assert!(
        node.document("show").await["scenes"]
            .as_array()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(std::fs::read_dir(data.join("applied")).unwrap().count(), 1);
    assert_eq!(node.stop().await, 0);

    // A marker whose restore/ is gone (the last move was the removal): nothing left to do
    std::fs::write(data.join("restore.pending"), b"restore/ is complete\n").unwrap();
    let node = Node::start(&home_b).await;
    assert!(!data.join("restore.pending").exists());
    assert_eq!(node.stop().await, 0);
}
