//! The Rust clampers give the outputs recorded from the Python ones
//! (RB-02 step 4): `tests/corpus/clamp_v2.json` for v2 documents and the
//! RB-01 `tests/corpus/sanitize_config.json` for version-1 configs.

use folo_protocol::clamp::{ClampError, clamp};
use folo_protocol::number::Num;
use serde_json::Value;

const CLAMP_V2: &str = include_str!("../../../tests/corpus/clamp_v2.json");

fn numbered() -> impl FnMut() -> String {
    let mut count = 0;
    move || {
        count += 1;
        format!("new{count:04}")
    }
}

fn is_integer_literal(n: &serde_json::Number) -> bool {
    !n.to_string().contains(['.', 'e', 'E'])
}

/// JSON equality as Python's: integers and floats are different kinds of
/// number, and numbers compare by value (`1e-05` is `1e-5`).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            is_integer_literal(x) == is_integer_literal(y) && Num::of(x).equals(&Num::of(y))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The first place two values differ, for the failure message.
fn difference(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (p, q))| difference(p, q, &format!("{path}[{i}]"))),
        (Value::Object(x), Value::Object(y)) => {
            for (key, value) in x {
                match y.get(key) {
                    Some(other) => {
                        if let Some(found) = difference(value, other, &format!("{path}.{key}")) {
                            return Some(found);
                        }
                    }
                    None => return Some(format!("{path}.{key}: only in Rust's output")),
                }
            }
            y.keys()
                .find(|k| !x.contains_key(*k))
                .map(|k| format!("{path}.{k}: only in Python's output"))
        }
        _ if same(a, b) => None,
        _ => Some(format!("{path}: Rust {a} != Python {b}")),
    }
}

#[test]
fn clamp_v2_corpus() {
    let corpus: Value = serde_json::from_str(CLAMP_V2).unwrap();
    let entries = corpus["entries"].as_array().unwrap();
    assert!(entries.len() >= 100);
    let mut failures = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let definition = entry["definition"].as_str().unwrap();
        let mut ids = numbered();
        let output = match clamp(definition, &entry["input"], &mut ids) {
            Ok(value) => value,
            Err(ClampError::Structural(_)) => serde_json::json!({"error": "structural"}),
            Err(e) => panic!("entry {index}: {e}"),
        };
        if let Some(found) = difference(&output, &entry["output"], definition) {
            failures.push(format!("entry {index}: {found}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} entries differ:\n{}",
        failures.len(),
        entries.len(),
        failures
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

const SANITIZE_V1: &str = include_str!("../../../tests/corpus/sanitize_config.json");

#[test]
fn sanitize_config_corpus() {
    let corpus: Value = serde_json::from_str(SANITIZE_V1).unwrap();
    let entries = corpus["entries"].as_array().unwrap();
    assert!(entries.len() >= 100);
    let mut failures = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut ids = numbered();
        let output = folo_protocol::v1::sanitize_config(
            &entry["input"],
            Some(&entry["led_count"]),
            &mut ids,
        );
        if let Some(found) = difference(&output, &entry["output"], "config") {
            failures.push(format!("entry {index}: {found}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} entries differ:\n{}",
        failures.len(),
        entries.len(),
        failures
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Every clamped document deserializes into its generated type.
fn deserializes(definition: &str, value: &Value) -> Result<(), String> {
    use folo_protocol::generated as g;
    let value = value.clone();
    let result = match definition {
        "ZonesDocument" => serde_json::from_value::<g::ZonesDocument>(value).map(drop),
        "ShowDocument" => serde_json::from_value::<g::ShowDocument>(value).map(drop),
        "ProvidersDocument" => serde_json::from_value::<g::ProvidersDocument>(value).map(drop),
        "SourcesDocument" => serde_json::from_value::<g::SourcesDocument>(value).map(drop),
        "ClientsDocument" => serde_json::from_value::<g::ClientsDocument>(value).map(drop),
        "SecretsDocument" => serde_json::from_value::<g::SecretsDocument>(value).map(drop),
        "NodeSettings" => serde_json::from_value::<g::NodeSettings>(value).map(drop),
        "PackManifest" => serde_json::from_value::<g::PackManifest>(value).map(drop),
        "PaletteItem" => serde_json::from_value::<g::PaletteItem>(value).map(drop),
        "ReactionItem" => serde_json::from_value::<g::ReactionItem>(value).map(drop),
        "LookItem" => serde_json::from_value::<g::LookItem>(value).map(drop),
        "SceneItem" => serde_json::from_value::<g::SceneItem>(value).map(drop),
        "ZoneTemplateItem" => serde_json::from_value::<g::ZoneTemplateItem>(value).map(drop),
        other => return Err(format!("no generated type mapped for {other}")),
    };
    result.map_err(|e| e.to_string())
}

#[test]
fn clamped_documents_fit_the_generated_types() {
    let corpus: Value = serde_json::from_str(CLAMP_V2).unwrap();
    let mut failures = Vec::new();
    for (index, entry) in corpus["entries"].as_array().unwrap().iter().enumerate() {
        if entry["output"].get("error").is_some() {
            continue;
        }
        let definition = entry["definition"].as_str().unwrap();
        if let Err(e) = deserializes(definition, &entry["output"]) {
            failures.push(format!("entry {index} ({definition}): {e}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
