//! Packs built in memory for the library's tests.

#![allow(dead_code, clippy::needless_pass_by_value)]

use std::path::PathBuf;

use folo_library::archive::{self, Entries};
use folo_library::canonical::hex;
use folo_library::{ItemType, Library};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A fresh folder for a test's library.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("folo-library-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn open(dir: &std::path::Path, version: &str) -> Library {
    Library::open(dir.join("library"), version.parse().unwrap())
}

pub fn palette(id: &str) -> Value {
    json!({"id": id, "schema_version": 2, "name": id, "colors": ["#0033CC", "#FF3399"]})
}

pub fn scene(id: &str) -> Value {
    json!({"id": id, "schema_version": 2, "scene": {"id": id, "name": id}, "led_count": 0})
}

/// The entries of a pack holding one palette and one scene (`item-a`, `item-b`).
pub fn entries(id: &str, version: &str) -> Entries {
    build(
        id,
        version,
        json!({}),
        &[
            (ItemType::Palettes, "p1", palette("item-a")),
            (ItemType::Scenes, "s1", scene("item-b")),
        ],
    )
}

/// A pack's entries from `(type, file stem, item)`; `extra` goes into the manifest.
pub fn build(id: &str, version: &str, extra: Value, items: &[(ItemType, &str, Value)]) -> Entries {
    let mut files = Entries::new();
    let mut contents = serde_json::Map::new();
    for (kind, stem, item) in items {
        let path = format!("{}/{stem}.json", kind.folder());
        files.insert(path, serde_json::to_vec_pretty(item).unwrap());
        contents
            .entry(kind.contents_key())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(item["id"].clone());
    }
    let hashes: serde_json::Map<String, Value> = files
        .iter()
        .map(|(p, b)| (p.clone(), Value::String(hex(&Sha256::digest(b)))))
        .collect();
    let mut manifest = json!({
        "id": id, "version": version, "folo": ">=1.0 <2", "name": "Test pack",
        "description": "A pack for tests.", "license": "MIT", "authors": ["tester"],
        "contents": contents, "files": hashes,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        manifest[key] = value;
    }
    files.insert(
        "folo-pack.json".to_owned(),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    );
    files
}

pub fn zip(entries: &Entries) -> Vec<u8> {
    archive::write(entries).unwrap()
}

/// The entries with the manifest's `files` recomputed after changing them.
pub fn rehash(mut entries: Entries) -> Entries {
    let mut manifest: Value = serde_json::from_slice(&entries["folo-pack.json"]).unwrap();
    let hashes: serde_json::Map<String, Value> = entries
        .iter()
        .filter(|(p, _)| p.as_str() != "folo-pack.json")
        .map(|(p, b)| (p.clone(), Value::String(hex(&Sha256::digest(b)))))
        .collect();
    manifest["files"] = Value::Object(hashes);
    entries.insert(
        "folo-pack.json".to_owned(),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    );
    entries
}

pub fn code(error: &folo_library::LibraryError) -> &'static str {
    error.code()
}
