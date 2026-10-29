//! The effect catalog (ADR-0035 D3): the built-in effects as the show
//! schema lists them (`x-folo-effects`), each with its parameter schema
//! (`x-folo-params-by-effect`) with its references resolved, so it stands
//! alone. The node serves it as `/api/v1/effects`.

use serde_json::{Map, Value};

use crate::clamp::DEFS;

/// How deep references may nest; the bundled schemas have no cycles.
const MAX_DEPTH: usize = 32;

/// The built-in effects, as api.json's `EffectInfo`, in the contract's order.
///
/// # Panics
///
/// Only if the bundled show schema lacks the catalog, which the contract
/// tests rule out.
#[must_use]
pub fn effects() -> Vec<Value> {
    let params = &DEFS["Layer"]["properties"]["params"];
    let by_effect = params["x-folo-params-by-effect"]
        .as_object()
        .expect("the show schema maps effects to their parameters");
    let catalog = params["x-folo-effects"]
        .as_object()
        .expect("the show schema lists the built-in effects");
    catalog
        .iter()
        .map(|(id, entry)| {
            let reference = by_effect[id].as_str().unwrap_or_default();
            let mut info = Map::new();
            info.insert("id".into(), Value::String(id.clone()));
            for key in ["version", "name", "description", "inputs", "min_elements"] {
                info.insert(key.into(), entry[key].clone());
            }
            info.insert("params_schema".into(), inline(&reference_to(reference), 0));
            Value::Object(info)
        })
        .collect()
}

/// A `$ref` to a bundled definition, as a schema.
fn reference_to(reference: &str) -> Value {
    let mut schema = Map::new();
    schema.insert("$ref".into(), Value::String(reference.to_owned()));
    Value::Object(schema)
}

/// `schema` with every `$ref` replaced by its definition; keywords beside a
/// `$ref` win over the definition's, as the clampers read them.
fn inline(schema: &Value, depth: usize) -> Value {
    assert!(depth < MAX_DEPTH, "the bundled schemas nest too deep");
    match schema {
        Value::Object(object) => {
            let mut out = Map::new();
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let name = reference.trim_start_matches("#/$defs/");
                let target = DEFS
                    .get(name)
                    .unwrap_or_else(|| panic!("no bundled definition {name}"));
                if let Value::Object(resolved) = inline(target, depth + 1) {
                    out = resolved;
                }
            }
            for (key, value) in object {
                if key != "$ref" {
                    out.insert(key.clone(), inline(value, depth + 1));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| inline(v, depth + 1)).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(value: &Value) -> usize {
        match value {
            Value::Object(object) => {
                usize::from(object.contains_key("$ref")) + object.values().map(refs).sum::<usize>()
            }
            Value::Array(items) => items.iter().map(refs).sum(),
            _ => 0,
        }
    }

    #[test]
    fn every_built_in_effect_is_listed_with_a_self_contained_schema() {
        let effects = effects();
        let ids: Vec<&str> = effects.iter().filter_map(|e| e["id"].as_str()).collect();
        assert_eq!(
            ids,
            [
                "audio-visualizer",
                "brightness",
                "mirror",
                "blur",
                "trail",
                "screen"
            ]
        );
        for effect in &effects {
            assert_eq!(refs(&effect["params_schema"]), 0, "{}", effect["id"]);
            assert_eq!(effect["params_schema"]["type"], "object");
            for key in ["version", "name", "description", "inputs", "min_elements"] {
                assert!(!effect[key].is_null(), "{} lacks {key}", effect["id"]);
            }
        }
        // The palette's colours come from common.json, resolved in place
        let palette = &effects[0]["params_schema"]["properties"]["palette"];
        assert_eq!(palette["type"], "array");
        assert!(palette["items"]["pattern"].is_string(), "{palette}");
    }
}
