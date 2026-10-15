//! The clamper (RB-02 step 4): makes any JSON value a valid FOLO document by
//! the rules in docs/specs/config-model.md#clamping-rules, driven by the
//! bundled schemas. A port of `folo_source/contracts/clamp.py`; the shared corpus
//! (`tests/corpus/clamp_v2.json`) checks that they agree.

use std::sync::LazyLock;

use serde_json::{Map, Value};

use crate::number::{self, Num};

const BUNDLE: &str = include_str!("../../../specs/generated/folo.schema.json");

/// The document format version this clamper reads.
pub const SCHEMA_VERSION: i64 = 2;

static DEFS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    let bundle: Value = serde_json::from_str(BUNDLE).expect("the schema bundle is valid JSON");
    bundle["$defs"]
        .as_object()
        .expect("the bundle has $defs")
        .clone()
});

/// Why a document can't be clamped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClampError {
    /// Its root isn't an object, or it's from a newer FOLO.
    Structural(String),
    /// No schema definition has that name.
    UnknownDefinition(String),
}

impl std::fmt::Display for ClampError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Structural(why) => write!(f, "structural error: {why}"),
            Self::UnknownDefinition(name) => write!(f, "no schema definition {name}"),
        }
    }
}

impl std::error::Error for ClampError {}

/// A new id: 8 lowercase hexadecimal characters, as today.
pub type NewId<'a> = &'a mut dyn FnMut() -> String;

/// `value` clamped to the schema definition `definition` (a document such as
/// `ZonesDocument`, or any entity).
///
/// # Errors
///
/// [`ClampError::Structural`] when the value isn't an object or its
/// `schema_version` is newer than [`SCHEMA_VERSION`].
///
/// # Panics
///
/// Only if the bundled schemas use a construct the clamper doesn't handle,
/// which the corpus tests rule out.
pub fn clamp(definition: &str, value: &Value, new_id: NewId<'_>) -> Result<Value, ClampError> {
    let schema = DEFS
        .get(definition)
        .ok_or_else(|| ClampError::UnknownDefinition(definition.to_owned()))?;
    let Value::Object(object) = value else {
        return Err(ClampError::Structural(format!(
            "{definition} must be a JSON object"
        )));
    };
    let versioned = schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key("schema_version"));
    if versioned
        && let Some(Value::Number(version)) = object.get("schema_version")
        && Num::of(version).greater_than(SCHEMA_VERSION)
    {
        return Err(ClampError::Structural(format!(
            "{definition} is schema version {version}; this FOLO reads {SCHEMA_VERSION}"
        )));
    }
    let mut ctx = Context { new_id };
    Ok(clamp_value(schema, Some(value), &mut ctx).expect("a document always clamps to a value"))
}

struct Context<'a, 'b> {
    new_id: &'a mut (dyn FnMut() -> String + 'b),
}

/// A schema with its `$ref` followed; keywords beside the `$ref` win.
fn effective(schema: &Value) -> Map<String, Value> {
    let mut schema = schema.as_object().cloned().unwrap_or_default();
    while let Some(Value::String(reference)) = schema.get("$ref") {
        let name = reference.trim_start_matches("#/$defs/");
        let mut target = DEFS
            .get(name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, value) in &schema {
            if key != "$ref" {
                target.insert(key.clone(), value.clone());
            }
        }
        schema = target;
    }
    schema
}

fn fallback(schema: &Map<String, Value>) -> Option<Value> {
    schema.get("default").cloned()
}

/// `None` is "missing": the property is left out, the item dropped.
fn clamp_value(schema: &Value, value: Option<&Value>, ctx: &mut Context<'_, '_>) -> Option<Value> {
    let schema = effective(schema);
    let value = value?;
    if let Some(Value::Array(options)) = schema.get("oneOf") {
        let kind = value.as_object().and_then(|o| o.get("kind"));
        if let Some(branch) = kind.and_then(|k| branch(options, k)) {
            return clamp_value(&Value::Object(branch), Some(value), ctx);
        }
        let default = fallback(&schema)?;
        return clamp_value(&Value::Object(schema.clone()), Some(&default), ctx);
    }
    if let Some(constant) = schema.get("const") {
        return Some(constant.clone());
    }
    if let Some(Value::Array(options)) = schema.get("enum") {
        return enum_match(value, options).or_else(|| fallback(&schema));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => clamp_object(&schema, value, ctx),
        Some("array") => clamp_array(&schema, value, ctx),
        Some("string") => clamp_string(&schema, value, ctx),
        Some(kind @ ("number" | "integer")) => clamp_number(&schema, value, kind == "integer"),
        Some("boolean") => {
            if value.is_boolean() {
                Some(value.clone())
            } else {
                fallback(&schema)
            }
        }
        _ => panic!("the clamper does not handle this schema: {schema:?}"),
    }
}

fn branch(options: &[Value], kind: &Value) -> Option<Map<String, Value>> {
    options.iter().map(effective).find(|branch| {
        branch
            .get("properties")
            .and_then(|p| p.get("kind"))
            .and_then(|k| k.get("const"))
            == Some(kind)
    })
}

fn enum_match(value: &Value, options: &[Value]) -> Option<Value> {
    options
        .iter()
        .find(|option| match (option, value) {
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => Num::of(a).equals(&Num::of(b)),
            _ => false,
        })
        .cloned()
}

fn bound(schema: &Map<String, Value>, key: &str) -> Option<f64> {
    schema.get(key).and_then(Value::as_f64)
}

fn clamp_number(schema: &Map<String, Value>, value: &Value, integer: bool) -> Option<Value> {
    let Value::Number(n) = value else {
        return fallback(schema);
    };
    let num = Num::of(n);
    if !num.is_finite_or_integer() {
        return fallback(schema);
    }
    let num = if integer { num.round_ties_even() } else { num };
    let num = num.clamp(bound(schema, "minimum"), bound(schema, "maximum"));
    Some(Value::Number(if integer {
        num.to_integer()
    } else {
        number::from_f64(num.to_f64())
    }))
}

fn clamp_string(
    schema: &Map<String, Value>,
    value: &Value,
    ctx: &mut Context<'_, '_>,
) -> Option<Value> {
    let mut text = match value {
        Value::String(s)
            if schema
                .get("pattern")
                .and_then(Value::as_str)
                .is_none_or(|p| full_match(p, s)) =>
        {
            s.clone()
        }
        _ => match fallback(schema)? {
            Value::String(s) => s,
            other => return Some(other),
        },
    };
    if schema.get("x-folo-color").and_then(Value::as_bool) == Some(true) {
        text = text.to_ascii_uppercase();
    }
    if let Some(limit) = schema.get("maxLength").and_then(Value::as_u64) {
        text = text
            .chars()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect();
    }
    if schema.get("x-folo-id").and_then(Value::as_bool) == Some(true) && text.is_empty() {
        text = (ctx.new_id)();
    }
    Some(Value::String(text))
}

fn full_match(pattern: &str, text: &str) -> bool {
    let anchored = format!("^(?:{pattern})$");
    regress::Regex::new(&anchored).is_ok_and(|re| re.find(text).is_some())
}

#[allow(clippy::too_many_lines)]
fn clamp_object(
    schema: &Map<String, Value>,
    value: &Value,
    ctx: &mut Context<'_, '_>,
) -> Option<Value> {
    let Value::Object(input) = value else {
        let default = fallback(schema)?;
        return clamp_object(schema, &default, ctx);
    };
    let extra = schema.get("additionalProperties");
    let Some(Value::Object(properties)) = schema.get("properties") else {
        if let Some(extra @ Value::Object(_)) = extra {
            // A map: every value clamped
            let mut out = Map::new();
            for (key, item) in input {
                if let Some(clamped) = clamp_value(extra, Some(item), ctx) {
                    out.insert(key.clone(), clamped);
                }
            }
            return Some(Value::Object(out));
        }
        return Some(value.clone()); // free-form (effect parameters of an unknown effect)
    };
    let mut out = Map::new();
    for (name, prop) in properties {
        let mut prop = effective(prop);
        let mut raw = input.get(name).cloned();
        if let Some(Value::String(source)) = prop.get("x-folo-kind-from") {
            let kind = out.get(source).cloned().unwrap_or(Value::Null);
            let matches = raw
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|o| o.get("kind"))
                == Some(&kind);
            if !matches {
                let mut tag = Map::new();
                tag.insert("kind".to_owned(), kind);
                raw = Some(Value::Object(tag));
            }
        }
        if let Some(Value::Object(by_effect)) = prop.get("x-folo-params-by-effect").cloned() {
            let effect = out
                .get("effect")
                .and_then(|e| e.get("id"))
                .and_then(Value::as_str);
            if let Some(reference) = effect.and_then(|e| by_effect.get(e)) {
                let mut params = Map::new();
                params.insert("$ref".to_owned(), reference.clone());
                params.insert("default".to_owned(), Value::Object(Map::new()));
                prop = params;
            }
        }
        let prop_value = Value::Object(prop.clone());
        let clamped = match raw {
            None => match fallback(&prop) {
                Some(default) => clamp_value(&prop_value, Some(&default), ctx),
                None => None,
            },
            Some(raw) => clamp_value(&prop_value, Some(&raw), ctx).or_else(|| {
                let default = fallback(&prop)?;
                clamp_value(&prop_value, Some(&default), ctx)
            }),
        };
        if let Some(clamped) = clamped {
            out.insert(name.clone(), clamped);
        }
    }
    if let Some(Value::Array(rules)) = schema.get("x-folo-rules") {
        for rule in rules {
            match rule.as_str() {
                Some("band") => rule_band(schema, &mut out),
                Some("thresholds") => rule_thresholds(schema, input, &mut out),
                Some("range") => rule_range(&mut out),
                other => panic!("unknown clamping rule {other:?}"),
            }
        }
    }
    for (name, prop) in properties {
        let decimals = effective(prop)
            .get("x-folo-decimals")
            .and_then(Value::as_i64);
        if let (Some(decimals), Some(Value::Number(n))) = (decimals, out.get(name)) {
            let rounded = number::py_round(Num::of(n).to_f64(), decimals);
            out.insert(name.clone(), Value::Number(number::from_f64(rounded)));
        }
    }
    Some(Value::Object(out))
}

fn clamp_array(
    schema: &Map<String, Value>,
    value: &Value,
    ctx: &mut Context<'_, '_>,
) -> Option<Value> {
    let Value::Array(input) = value else {
        let default = fallback(schema)?;
        return clamp_array(schema, &default, ctx);
    };
    let items_schema = schema.get("items").cloned().unwrap_or(Value::Null);
    let mut out: Vec<Value> = input
        .iter()
        .filter_map(|item| clamp_value(&items_schema, Some(item), ctx))
        .collect();
    if let Some(limit) = schema.get("maxItems").and_then(Value::as_u64) {
        out.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    // Ids are unique within a list
    if has_id(&items_schema) {
        let mut seen = std::collections::HashSet::new();
        for item in &mut out {
            if let Some(Value::String(id)) = item.get("id").cloned() {
                if seen.contains(&id) {
                    let fresh = (ctx.new_id)();
                    item["id"] = Value::String(fresh.clone());
                    seen.insert(fresh);
                } else {
                    seen.insert(id);
                }
            }
        }
    }
    let minimum = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
    if (out.len() as u64) < minimum {
        let default = fallback(schema)?;
        return clamp_array(schema, &default, ctx);
    }
    Some(Value::Array(out))
}

fn has_id(schema: &Value) -> bool {
    let schema = effective(schema);
    schema
        .get("properties")
        .and_then(|p| p.get("id"))
        .is_some_and(|id| effective(id).get("x-folo-id").and_then(Value::as_bool) == Some(true))
}

// ------------------------------------------------------------ cross-field rules

fn float_of(out: &Map<String, Value>, name: &str) -> f64 {
    out.get(name).and_then(Value::as_f64).unwrap_or(0.0)
}

/// `freq_lo` <= `freq_hi`, at least the ratio apart (`sanitize_segment`'s rule).
fn rule_band(schema: &Map<String, Value>, out: &mut Map<String, Value>) {
    let band = &schema["x-folo-band"];
    let low = band["low"].as_str().unwrap_or("freq_lo");
    let high = band["high"].as_str().unwrap_or("freq_hi");
    let ratio = band["ratio"].as_f64().unwrap_or(1.0);
    let top = schema["properties"]
        .get(high)
        .map(effective)
        .and_then(|p| p.get("maximum").and_then(Value::as_f64))
        .unwrap_or(f64::INFINITY);
    let (mut lo, mut hi) = (float_of(out, low), float_of(out, high));
    if hi < lo {
        std::mem::swap(&mut lo, &mut hi);
    }
    if hi < lo * ratio {
        hi = top.min(lo * ratio);
        lo = hi / ratio;
    }
    out.insert(low.to_owned(), Value::Number(number::from_f64(lo)));
    out.insert(high.to_owned(), Value::Number(number::from_f64(hi)));
}

fn limited(value: Option<&Value>, limits: &Value) -> f64 {
    let lo = limits["minimum"].as_f64().unwrap_or(f64::NEG_INFINITY);
    let hi = limits["maximum"].as_f64().unwrap_or(f64::INFINITY);
    match value {
        Some(Value::Number(n)) if Num::of(n).is_finite_or_integer() => {
            Num::of(n).clamp(Some(lo), Some(hi)).to_f64()
        }
        _ => limits["default"].as_f64().unwrap_or(0.0),
    }
}

/// threshold and ceiling in the units of the trigger, ceiling at least `gap` above.
fn rule_thresholds(
    schema: &Map<String, Value>,
    raw: &Map<String, Value>,
    out: &mut Map<String, Value>,
) {
    let spec = &schema["x-folo-thresholds"];
    let trigger_field = spec["trigger"].as_str().unwrap_or("trigger");
    let trigger = out.get(trigger_field).and_then(Value::as_str).unwrap_or("");
    let limits = spec["by_trigger"]
        .get(trigger)
        .unwrap_or(&spec["otherwise"]);
    let threshold = limited(raw.get("threshold"), &limits["threshold"]);
    let mut ceiling = limited(raw.get("ceiling"), &limits["ceiling"]);
    let gap = spec["gap"].as_f64().unwrap_or(0.0);
    if ceiling < threshold + gap {
        ceiling = threshold + gap;
    }
    out.insert(
        "threshold".to_owned(),
        Value::Number(number::from_f64(threshold)),
    );
    out.insert(
        "ceiling".to_owned(),
        Value::Number(number::from_f64(ceiling)),
    );
}

/// start <= end.
fn rule_range(out: &mut Map<String, Value>) {
    let (Some(start), Some(end)) = (out.get("start").cloned(), out.get("end").cloned()) else {
        return;
    };
    let (Some(s), Some(e)) = (start.as_f64(), end.as_f64()) else {
        return;
    };
    if e < s {
        out.insert("start".to_owned(), end);
        out.insert("end".to_owned(), start);
    }
}

#[cfg(test)]
mod tests {
    use super::{ClampError, clamp};
    use serde_json::json;

    fn numbered() -> impl FnMut() -> String {
        let mut count = 0;
        move || {
            count += 1;
            format!("new{count:04}")
        }
    }

    #[test]
    fn structural_errors() {
        let mut ids = numbered();
        assert!(matches!(
            clamp("ZonesDocument", &json!([]), &mut ids),
            Err(ClampError::Structural(_))
        ));
        assert!(matches!(
            clamp("ZonesDocument", &json!({"schema_version": 3}), &mut ids),
            Err(ClampError::Structural(_))
        ));
    }

    #[test]
    fn fills_defaults_and_clamps() {
        let mut ids = numbered();
        let zones = clamp(
            "ZonesDocument",
            &json!({"zones": [{"ambient": {"level": 3, "color": "red"}, "beat_offset_ms": 99.5}]}),
            &mut ids,
        )
        .unwrap();
        let zone = &zones["zones"][0];
        assert_eq!(zone["id"], "new0001");
        assert_eq!(zone["ambient"]["level"].as_f64(), Some(1.0));
        assert_eq!(zone["ambient"]["color"], "#4B00FF");
        assert_eq!(zone["beat_offset_ms"].as_i64(), Some(100));
        assert_eq!(zone["bounds"]["z"].as_f64(), Some(2.5));
    }
}
