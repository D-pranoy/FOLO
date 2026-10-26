//! Canonical JSON against the corpus Python is checked with too
//! (`tests/corpus/canonical.json`, `tests/test_canonical.py`).

use std::path::PathBuf;

use folo_library::canonical::{item_hash, to_canonical};
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus() -> Value {
    let text = std::fs::read_to_string(root().join("tests/corpus/canonical.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn the_cases_agree_with_python() {
    let corpus = corpus();
    let cases = corpus["cases"].as_array().unwrap();
    assert!(cases.len() > 20);
    for case in cases {
        let text = case["json"].as_str().unwrap();
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            to_canonical(&value).unwrap(),
            case["canonical"].as_str().unwrap(),
            "{text}"
        );
        assert_eq!(
            item_hash(&value).unwrap(),
            case["sha256"].as_str().unwrap(),
            "{text}"
        );
    }
}

#[test]
fn the_built_in_items_hash_as_python_does() {
    let corpus = corpus();
    let files = corpus["files"].as_array().unwrap();
    assert!(files.len() > 70);
    for file in files {
        let path = file["path"].as_str().unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(root().join(path)).unwrap()).unwrap();
        assert_eq!(
            item_hash(&value).unwrap(),
            file["sha256"].as_str().unwrap(),
            "{path}"
        );
    }
}

/// Clamping a built-in item changes nothing, so the library's hash of it
/// is the one Python computes from the committed file.
#[test]
fn the_library_hashes_the_built_ins_as_python_does() {
    let corpus = corpus();
    let python: std::collections::HashMap<&str, &str> = corpus["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap(), f["sha256"].as_str().unwrap()))
        .collect();
    let dir = std::env::temp_dir().join(format!("folo-library-corpus-{}", std::process::id()));
    let library = folo_library::Library::open(dir.join("library"), "0.0.0".parse().unwrap());
    let mut checked = 0;
    for kind in folo_library::ItemType::ALL {
        for item in library.items(kind) {
            let pack = item["pack"].as_str().unwrap();
            if !pack.starts_with("folo/") {
                continue;
            }
            let id = item["item"]["id"].as_str().unwrap();
            let path = format!("packs/{pack}/{}/{id}.json", kind.folder());
            assert_eq!(
                item["hash"].as_str().unwrap(),
                python[path.as_str()],
                "{path}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, python.len());
    let _ = std::fs::remove_dir_all(dir);
}
