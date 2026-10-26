//! Applying scenes and Reset all end to end, in this process (RB-09 change
//! 4): docs/specs/client-api.md#applying-scenes-and-reset-all, binding,
//! bindings, scaling, provenance, bases and `keep_changes`.

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
use folo_protocol::generated::SceneApplied;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-apply-api-{}", folo_store::new_id()));
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
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
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
    if_match: Option<u64>,
    body: &[u8],
) -> Answer {
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(revision) = if_match {
        head.push_str(&format!("If-Match: {revision}\r\n"));
    }
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
    Answer { status, body }
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
        if_match: Option<u64>,
        body: Vec<u8>,
    ) -> Answer {
        let (port, method, path, content_type) = (
            self.port,
            method.to_owned(),
            path.to_owned(),
            content_type.map(str::to_owned),
        );
        tokio::task::spawn_blocking(move || {
            exchange(
                port,
                &method,
                &path,
                content_type.as_deref(),
                if_match,
                &body,
            )
        })
        .await
        .unwrap()
    }

    async fn get(&self, path: &str) -> Answer {
        self.call("GET", path, None, None, Vec::new()).await
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

impl Node {
    async fn send(&self, method: &str, path: &str, body: &Value, if_match: Option<u64>) -> Answer {
        self.call(
            method,
            path,
            Some("application/json"),
            if_match,
            body.to_string().into_bytes(),
        )
        .await
    }

    async fn apply(&self, reference: &str, body: &Value) -> Answer {
        self.send(
            "POST",
            &format!("/api/v1/scenes/{reference}/apply"),
            body,
            None,
        )
        .await
    }

    async fn show(&self) -> Value {
        self.get("/api/v1/show").await.json()
    }

    async fn zones(&self) -> Value {
        self.get("/api/v1/zones").await.json()
    }

    async fn install(&self, entries: &Entries) {
        let bytes = archive::write(entries).unwrap();
        let answer = self
            .call(
                "POST",
                "/api/v1/library/packs",
                Some("application/zip"),
                None,
                bytes,
            )
            .await;
        assert_eq!(
            answer.status,
            201,
            "{}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

fn layer(id: &str, start: u64, end: u64) -> Value {
    json!({"id": id, "name": id, "effect": {"id": "audio-visualizer", "version": "1.0.0"},
        "region": {"kind": "cells", "start": start, "end": end}})
}

/// `acme/cells:bar`: cells made for 100 LEDs, in two roles.
fn cells_pack() -> Entries {
    let item = json!({"id": "bar", "schema_version": 2, "led_count": 100, "fallbacks": {},
    "scene": {"id": "bar", "name": "Bar", "layers_by_role": {
        "strip.main": [layer("a", 0, 49), layer("b", 50, 99)],
        "strip.extra": [layer("c", 0, 99)],
    }}});
    pack(
        "acme/cells",
        "1.0.0",
        &json!({}),
        &[("scenes", "bar", item)],
    )
}

fn surface(id: &str, kind: &str, role: &str, width: u64, height: u64) -> Value {
    json!({"id": id, "kind": kind, "role": role, "resolution": {"width": width, "height": height},
        "master": 0.4, "background": "#112233"})
}

async fn setup(node: &Node) {
    let zones = [
        json!({"id": "z1", "name": "Desk", "surfaces": [surface("s1", "strip", "strip.main", 30, 1)]}),
        json!({"id": "z2", "name": "Wall", "surfaces": [
            surface("s2", "strip", "strip.main", 144, 1),
            surface("c1", "canvas", "canvas.room", 8, 4),
        ]}),
    ];
    for zone in zones {
        let answer = node.send("POST", "/api/v1/zones", &zone, None).await;
        assert_eq!(
            answer.status,
            201,
            "{}",
            String::from_utf8_lossy(&answer.body)
        );
    }
    node.install(&cells_pack()).await;
}

fn find_file(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
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

fn base_path(home: &Home, scene: &str) -> Option<PathBuf> {
    find_file(&home.0, &folo_library::reference::base_file_name(scene))
}

fn scene_of<'a>(show: &'a Value, id: &str) -> &'a Value {
    show["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("no scene {id} in {show}"))
}

fn applied(answer: &Answer) -> (Value, Value) {
    assert_eq!(
        answer.status,
        200,
        "{}",
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    serde_json::from_value::<SceneApplied>(body.clone())
        .unwrap_or_else(|error| panic!("not a SceneApplied: {error}\n{body}"));
    (body["show"].clone(), body["results"][0].clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn applying_scenes() {
    let home = Home::new();
    let node = Node::start(&home).await;
    setup(&node).await;
    copying_a_built_in(&node, &home).await;
    binding_and_bindings(&node).await;
    refusals(&node).await;
    keeping_changes(&node, &home).await;
    assert_eq!(node.stop().await, 0);
}

async fn copying_a_built_in(node: &Node, home: &Home) {
    let before = node.show().await["revision"].as_u64().unwrap();
    // An alias, applied where the zone has no scene: a new scene, active
    let (show, result) = applied(&node.apply("genre:edm", &json!({"zone": "z1"})).await);
    assert!(
        show["revision"].as_u64().unwrap() > before,
        "the revision moves"
    );
    let first = result["scene"].as_str().unwrap().to_owned();
    assert_eq!(result["roles"], json!({"strip.main": "s1"}));
    assert_eq!(result["skipped"], json!([]));
    assert_eq!(show["active"]["z1"], first.as_str());
    let scene = scene_of(&show, &first);
    let provenance = &scene["provenance"];
    assert_eq!(provenance["pack"], "folo/genres");
    assert_eq!(provenance["item"], "edm");
    assert_eq!(provenance["hash"].as_str().unwrap().len(), 64);
    assert!(
        scene["layers_by_role"]["strip.main"]
            .as_array()
            .unwrap()
            .len()
            > 1
    );
    assert!(base_path(home, &first).is_some(), "the base is kept");

    // Again: in place, the same id and position
    let (show, result) = applied(&node.apply("genre:edm", &json!({"zone": "z1"})).await);
    assert_eq!(result["scene"], first.as_str());
    assert_eq!(show["scenes"].as_array().unwrap().len(), 1);

    // The full reference, percent-encoded, into the other zone: a new scene
    let (show, result) = applied(
        &node
            .apply("folo%2Fgenres%3Aedm", &json!({"zone": "z2"}))
            .await,
    );
    let second = result["scene"].as_str().unwrap().to_owned();
    assert_ne!(second, first);
    assert_eq!(show["scenes"].as_array().unwrap().len(), 2);

    // A scene another zone has active is not overwritten: z1 gets a new one
    let mut shared = show.clone();
    shared["active"]["z2"] = first.as_str().into();
    assert_eq!(
        node.send("PUT", "/api/v1/show", &shared, None).await.status,
        200
    );
    let (show, result) = applied(&node.apply("genre:edm", &json!({"zone": "z1"})).await);
    let third = result["scene"].as_str().unwrap();
    assert_ne!(third, first);
    assert_eq!(show["scenes"].as_array().unwrap().len(), 3);
    assert_eq!(show["active"]["z2"], first.as_str());
    assert_eq!(show["active"]["z1"], third);

    // A scene of the show only becomes active; nothing is copied
    let (show, result) = applied(&node.apply(&first, &json!({"zone": "z1"})).await);
    assert_eq!(result["scene"], first.as_str());
    assert_eq!(result["roles"], json!({}));
    assert_eq!(show["active"]["z1"], first.as_str());
    assert_eq!(show["scenes"].as_array().unwrap().len(), 3);
}

async fn binding_and_bindings(node: &Node) {
    // Cells scale to the surface: 0-49 of 100 LEDs is 0-14 of 30
    let (show, result) = applied(
        &node
            .apply("acme%2Fcells%3Abar", &json!({"zone": "z1"}))
            .await,
    );
    let scene = scene_of(&show, result["scene"].as_str().unwrap());
    // `strip.extra` is taken by the first role in byte order; `strip.main` has no surface left
    assert_eq!(result["roles"], json!({"strip.extra": "s1"}));
    assert_eq!(result["skipped"], json!(["strip.main"]));
    let region = &scene["layers_by_role"]["strip.main"][0]["region"];
    assert_eq!(*region, json!({"kind": "cells", "start": 0, "end": 29}));

    // z2: strip.extra takes the nearest strip, strip.main falls back to the room canvas
    let (show, result) = applied(
        &node
            .apply("acme%2Fcells%3Abar", &json!({"zone": "z2"}))
            .await,
    );
    assert_eq!(
        result["roles"],
        json!({"strip.extra": "s2", "strip.main": "c1"})
    );
    let scene = scene_of(&show, result["scene"].as_str().unwrap());
    let canvas = &scene["layers_by_role"]["canvas.room"][0]["region"];
    assert_eq!(*canvas, json!({"kind": "cells", "start": 0, "end": 3}));
    assert_eq!(scene["provenance"]["pack"], "acme/cells");

    // Overrides are remembered for this scene and zone
    let roles = json!({"strip.main": "s2", "strip.extra": "c1"});
    let (show, result) = applied(
        &node
            .apply("acme%2Fcells%3Abar", &json!({"zone": "z2", "roles": roles}))
            .await,
    );
    assert_eq!(result["roles"], roles);
    let bindings = show["bindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        bindings[0],
        json!({"zone": "z2", "scene": "acme/cells:bar", "roles": roles})
    );
    let (_, again) = applied(
        &node
            .apply("acme%2Fcells%3Abar", &json!({"zone": "z2"}))
            .await,
    );
    assert_eq!(again["roles"], roles, "remembered");

    // "" returns a role to automatic; none left removes the entry
    let (show, _) = applied(
        &node
            .apply(
                "acme%2Fcells%3Abar",
                &json!({"zone": "z2", "roles": {"strip.main": ""}}),
            )
            .await,
    );
    assert_eq!(show["bindings"][0]["roles"], json!({"strip.extra": "c1"}));
    let (show, _) = applied(
        &node
            .apply(
                "acme%2Fcells%3Abar",
                &json!({"zone": "z2", "roles": {"strip.extra": ""}}),
            )
            .await,
    );
    assert!(
        show["bindings"].as_array().is_none_or(Vec::is_empty),
        "{show}"
    );

    // A binding of a zone that goes is dropped at the next write
    let roles = json!({"strip.main": "s2"});
    node.apply("acme%2Fcells%3Abar", &json!({"zone": "z2", "roles": roles}))
        .await;
    let gone = node
        .send(
            "POST",
            "/api/v1/zones",
            &json!({"id": "z3", "surfaces": [surface("s3", "strip", "strip.main", 10, 1)]}),
            None,
        )
        .await;
    assert_eq!(gone.status, 201);
    let bound = node
        .apply(
            "acme%2Fcells%3Abar",
            &json!({"zone": "z3", "roles": {"strip.main": "s3"}}),
        )
        .await;
    assert_eq!(bound.status, 200);
    assert_eq!(node.show().await["bindings"].as_array().unwrap().len(), 2);
    let deleted = node
        .call(
            "DELETE",
            "/api/v1/zones/z3",
            Some("application/json"),
            None,
            Vec::new(),
        )
        .await;
    assert_eq!(deleted.status, 204);
    let (show, _) = applied(&node.apply("genre:edm", &json!({"zone": "z1"})).await);
    let zones: Vec<&str> = show["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["zone"].as_str().unwrap())
        .collect();
    assert_eq!(zones, ["z2"]);
}

async fn refusals(node: &Node) {
    let answer = node.apply("genre:edm", &json!({"zone": "nope"})).await;
    assert_eq!((answer.status, answer.code()), (404, "not_found".into()));
    assert_eq!(answer.details()["what"], "zone");
    for reference in ["nonsense", "acme%2Fcells%3Anone", "none%2Fnone%3Ax"] {
        let answer = node.apply(reference, &json!({"zone": "z1"})).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "not_found".into()),
            "{reference}"
        );
        assert_eq!(answer.details()["what"], "scene");
    }
    let show = node.show().await;
    let own = show["scenes"][0]["id"].as_str().unwrap().to_owned();
    for body in [
        json!({"zone": "z1", "roles": {"strip.main": "s1"}}),
        json!({"zone": "z1", "keep_changes": true}),
    ] {
        let answer = node.apply(&own, &body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "bad_body".into()),
            "{body}"
        );
    }
    for body in [
        json!({}),
        json!({"zone": 1}),
        json!({"zone": "z1", "extra": 1}),
        json!({"zone": "z1", "roles": {"strip.main": "no-such-surface"}}),
        json!({"zone": "z1", "roles": {"strip.main": 3}}),
        json!({"zone": "z1", "keep_changes": "yes"}),
    ] {
        let answer = node.apply("genre:edm", &body).await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "bad_body".into()),
            "{body}"
        );
    }

    // If-Match names the show's revision
    let revision = node.show().await["revision"].as_u64().unwrap();
    let stale = node
        .send(
            "POST",
            "/api/v1/scenes/genre%3Aedm/apply",
            &json!({"zone": "z1"}),
            Some(revision - 1),
        )
        .await;
    assert_eq!(
        (stale.status, stale.code()),
        (412, "revision_mismatch".into())
    );
    let fresh = node
        .send(
            "POST",
            "/api/v1/scenes/genre%3Aedm/apply",
            &json!({"zone": "z1"}),
            Some(revision),
        )
        .await;
    assert_eq!(fresh.status, 200);

    // A show with 256 scenes can't take a new one
    let mut full = node.show().await;
    let scenes: Vec<Value> = (0..256)
        .map(|n| json!({"id": format!("f{n}"), "name": "f"}))
        .collect();
    full["scenes"] = Value::Array(scenes);
    full["active"] = json!({"z1": "f0"});
    assert_eq!(
        node.send("PUT", "/api/v1/show", &full, None).await.status,
        200
    );
    let answer = node.apply("genre:edm", &json!({"zone": "z2"})).await;
    assert_eq!((answer.status, answer.code()), (409, "list_full".into()));
    // z1's scene is active only there, so an apply to it is in place and fits
    assert_eq!(
        node.apply("genre:edm", &json!({"zone": "z1"})).await.status,
        200
    );
}

async fn keeping_changes(node: &Node, home: &Home) {
    let mut show = node.show().await;
    show["scenes"] = json!([]);
    show["active"] = json!({});
    assert_eq!(
        node.send("PUT", "/api/v1/show", &show, None).await.status,
        200
    );

    // No scene, or one without provenance: nothing to merge against
    let answer = node
        .apply("genre:edm", &json!({"zone": "z1", "keep_changes": true}))
        .await;
    assert_eq!((answer.status, answer.code()), (409, "no_base".into()));
    let (show, result) = applied(&node.apply("genre:edm", &json!({"zone": "z1"})).await);
    let id = result["scene"].as_str().unwrap().to_owned();
    let other = node
        .apply(
            "genre:ambient",
            &json!({"zone": "z1", "keep_changes": true}),
        )
        .await;
    assert_eq!(
        (other.status, other.code()),
        (409, "no_base".into()),
        "another item has no base here"
    );

    // The user renames the scene, changes a parameter and removes a layer
    let mut mine = show.clone();
    let scene = mine["scenes"][0].as_object_mut().unwrap();
    scene.insert("name".into(), "Mine".into());
    let layers = scene["layers_by_role"]["strip.main"]
        .as_array_mut()
        .unwrap();
    let removed = layers.remove(1)["id"].as_str().unwrap().to_owned();
    layers[0]["params"]["gain"] = json!(2.5);
    layers[0]["name"] = "Kick".into();
    assert_eq!(
        node.send("PUT", "/api/v1/show", &mine, None).await.status,
        200
    );
    let kept = node.show().await;

    let (merged, result) = applied(
        &node
            .apply("genre:edm", &json!({"zone": "z1", "keep_changes": true}))
            .await,
    );
    assert_eq!(result["scene"], id.as_str());
    let scene = scene_of(&merged, &id);
    let after = scene_of(&kept, &id);
    assert_eq!(scene["name"], "Mine");
    let layers = scene["layers_by_role"]["strip.main"].as_array().unwrap();
    assert_eq!(layers[0]["name"], "Kick");
    assert_eq!(layers[0]["params"]["gain"], 2.5);
    assert!(
        layers.iter().all(|l| l["id"] != removed.as_str()),
        "a removed layer stays removed"
    );
    assert_eq!(
        layers.len(),
        after["layers_by_role"]["strip.main"]
            .as_array()
            .unwrap()
            .len()
    );
    assert_eq!(scene["provenance"], after["provenance"]);

    // The same merge again changes nothing: the merged scene's base is theirs
    let (again, _) = applied(
        &node
            .apply("genre:edm", &json!({"zone": "z1", "keep_changes": true}))
            .await,
    );
    assert_eq!(
        scene_of(&again, &id)["layers_by_role"],
        scene["layers_by_role"]
    );

    // Without keep_changes the item overwrites the changes
    let (plain, _) = applied(&node.apply("genre:edm", &json!({"zone": "z1"})).await);
    assert_eq!(scene_of(&plain, &id)["name"], "EDM festival");

    // Gone with the base file: no_base
    let path = base_path(home, &id).expect("the base");
    std::fs::remove_file(path).unwrap();
    let answer = node
        .apply("genre:edm", &json!({"zone": "z1", "keep_changes": true}))
        .await;
    assert_eq!((answer.status, answer.code()), (409, "no_base".into()));

    // The base goes with its scene, and with its provenance
    node.apply("genre:edm", &json!({"zone": "z1"})).await;
    assert!(base_path(home, &id).is_some());
    let mut stripped = node.show().await;
    stripped["scenes"][0]
        .as_object_mut()
        .unwrap()
        .remove("provenance");
    assert_eq!(
        node.send("PUT", "/api/v1/show", &stripped, None)
            .await
            .status,
        200
    );
    assert!(base_path(home, &id).is_none(), "no provenance, no base");
    node.apply("genre:edm", &json!({"zone": "z1"})).await;
    let mut emptied = node.show().await;
    emptied["scenes"] = json!([]);
    emptied["active"] = json!({});
    assert_eq!(
        node.send("PUT", "/api/v1/show", &emptied, None)
            .await
            .status,
        200
    );
    let left = std::fs::read_dir(find_applied(home)).map_or(0, Iterator::count);
    assert_eq!(left, 0, "the bases of removed scenes are removed");
}

fn find_applied(home: &Home) -> PathBuf {
    fn walk(dir: &std::path::Path) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "applied") {
                    return Some(path);
                }
                if let Some(found) = walk(&path) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(&home.0).unwrap_or_else(|| home.0.join("applied"))
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_all() {
    let home = Home::new();
    let node = Node::start(&home).await;
    setup(&node).await;
    let handle = &node;
    let reset =
        |body: Value| async move { handle.send("POST", "/api/v1/show/reset", &body, None).await };

    // Every zone, master 1 and a black background
    let zones_before = node.zones().await["revision"].as_u64().unwrap();
    let answer = reset(json!({})).await;
    assert_eq!(
        answer.status,
        200,
        "{}",
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    serde_json::from_value::<SceneApplied>(body.clone()).unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let show = &body["show"];
    let scene = scene_of(show, results[0]["scene"].as_str().unwrap());
    assert_eq!(scene["provenance"]["pack"], "folo/core");
    assert_eq!(scene["provenance"]["item"], "default");
    assert_eq!(
        scene["layers_by_role"]["strip.main"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let zones = node.zones().await;
    assert!(zones["revision"].as_u64().unwrap() > zones_before);
    for zone in zones["zones"].as_array().unwrap() {
        for surface in zone["surfaces"].as_array().unwrap() {
            assert_eq!(surface["master"].as_f64(), Some(1.0), "{surface}");
            assert_eq!(surface["background"], "#000000");
        }
    }
    // The canvas has no strip role of its own, so the default's `strip.main` goes to the strip
    assert_eq!(results[1]["roles"], json!({"strip.main": "s2"}));

    // Nothing differs now: zones.json isn't written again
    let revision = zones["revision"].as_u64().unwrap();
    assert_eq!(reset(json!({})).await.status, 200);
    assert_eq!(node.zones().await["revision"].as_u64().unwrap(), revision);

    // An empty body is every zone; listed zones are the only ones
    let empty = node
        .call(
            "POST",
            "/api/v1/show/reset",
            Some("application/json"),
            None,
            Vec::new(),
        )
        .await;
    assert_eq!(empty.status, 200);
    let one = reset(json!({"zones": ["z1"]})).await.json();
    assert_eq!(one["results"].as_array().unwrap().len(), 1);

    // default_scene: a built-in id, then something that names nothing
    let mut show = node.show().await;
    show["default_scene"] = "genre:edm".into();
    assert_eq!(
        node.send("PUT", "/api/v1/show", &show, None).await.status,
        200
    );
    let edm = reset(json!({"zones": ["z1"]})).await.json();
    let id = edm["results"][0]["scene"].as_str().unwrap();
    assert_eq!(scene_of(&edm["show"], id)["provenance"]["item"], "edm");
    show["default_scene"] = "nothing-here".into();
    assert_eq!(
        node.send("PUT", "/api/v1/show", &show, None).await.status,
        200
    );
    let fallback = reset(json!({"zones": ["z1"]})).await.json();
    let id = fallback["results"][0]["scene"].as_str().unwrap();
    assert_eq!(
        scene_of(&fallback["show"], id)["provenance"]["item"],
        "default"
    );

    // An unknown zone refuses the whole reset; nothing changes
    let revision = node.show().await["revision"].clone();
    let answer = reset(json!({"zones": ["z1", "ghost"]})).await;
    assert_eq!((answer.status, answer.code()), (404, "not_found".into()));
    assert_eq!(answer.details()["what"], "zone");
    assert_eq!(node.show().await["revision"], revision);
    let bad = reset(json!({"zones": "z1"})).await;
    assert_eq!((bad.status, bad.code()), (400, "bad_body".into()));
    let stale = node
        .send("POST", "/api/v1/show/reset", &json!({}), Some(0))
        .await;
    assert_eq!(
        (stale.status, stale.code()),
        (412, "revision_mismatch".into())
    );
    assert_eq!(node.stop().await, 0);
}
