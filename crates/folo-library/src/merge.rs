//! Re-applying a scene, keeping the user's changes: a three-way merge of the
//! scene as it was applied (base), as it is now (mine) and as an apply would
//! write it now (theirs) (ADR-0038 D15).

use serde_json::{Map, Value};

/// One value of a three-way merge: what mine changed is kept, the rest takes
/// theirs. An absent value is `None`.
fn pick<'a>(
    base: Option<&'a Value>,
    mine: Option<&'a Value>,
    theirs: Option<&'a Value>,
) -> Option<&'a Value> {
    if mine == base { theirs } else { mine }
}

/// The union of the keys of `maps`, theirs first.
fn keys<'a>(maps: &[&'a Map<String, Value>]) -> Vec<&'a String> {
    let mut out: Vec<&String> = Vec::new();
    for map in maps {
        for key in map.keys() {
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out
}

fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The fields of an entry (a layer or a macro) merged; `params` merges key by key.
fn merge_entry(base: &Value, mine: &Value, theirs: &Value) -> Value {
    let (base, mine, theirs) = (object(Some(base)), object(Some(mine)), object(Some(theirs)));
    let mut out = Map::new();
    for key in keys(&[&theirs, &mine, &base]) {
        let picked = if key == "params" {
            let merged = merge_fields(
                &object(base.get(key)),
                &object(mine.get(key)),
                &object(theirs.get(key)),
            );
            Some(Value::Object(merged))
        } else {
            pick(base.get(key), mine.get(key), theirs.get(key)).cloned()
        };
        if let Some(value) = picked {
            out.insert(key.clone(), value);
        }
    }
    Value::Object(out)
}

fn merge_fields(
    base: &Map<String, Value>,
    mine: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Map<String, Value> {
    let mut out = Map::new();
    for key in keys(&[theirs, mine, base]) {
        if let Some(value) = pick(base.get(key), mine.get(key), theirs.get(key)) {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

fn by_id<'a>(entries: &'a [Value], id: &str) -> Option<&'a Value> {
    entries.iter().find(|entry| entry["id"] == id)
}

fn id_of(entry: &Value) -> &str {
    entry["id"].as_str().unwrap_or_default()
}

/// Layers or macros merged by id: ordered as theirs orders them, then mine's own.
fn merge_list(base: &[Value], mine: &[Value], theirs: &[Value]) -> Vec<Value> {
    let empty = Value::Object(Map::new());
    let mut out = Vec::new();
    for entry in theirs {
        let id = id_of(entry);
        let (in_base, in_mine) = (by_id(base, id), by_id(mine, id));
        if in_base.is_some() && in_mine.is_none() {
            // Mine removed it, and it stays removed
            continue;
        }
        let merged = match in_mine {
            Some(mine) => merge_entry(in_base.unwrap_or(&empty), mine, entry),
            None => entry.clone(),
        };
        out.push(merged);
    }
    for entry in mine {
        let id = id_of(entry);
        if by_id(theirs, id).is_some() {
            continue;
        }
        // Theirs dropped it: it goes only when mine left it as it was
        if by_id(base, id) != Some(entry) {
            out.push(entry.clone());
        }
    }
    out
}

fn list(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

/// The merged `layers_by_role`.
fn merge_roles(base: &Value, mine: &Value, theirs: &Value) -> Value {
    let (base, mine, theirs) = (object(Some(base)), object(Some(mine)), object(Some(theirs)));
    let mut out = Map::new();
    for role in keys(&[&theirs, &mine, &base]) {
        let layers = merge_list(
            &list(base.get(role)),
            &list(mine.get(role)),
            &list(theirs.get(role)),
        );
        // A role mine emptied stays there; one only in base and gone from both doesn't
        if !layers.is_empty() || mine.contains_key(role) {
            out.insert(role.clone(), Value::Array(layers));
        }
    }
    Value::Object(out)
}

/// The scene to write: mine, taking what theirs changed where mine left base
/// alone, with theirs' provenance. Its `id` is mine's; the caller sets the
/// id it writes under.
#[must_use]
pub fn merge(base: &Value, mine: &Value, theirs: &Value) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), mine["id"].clone());
    for key in ["name", "tags"] {
        if let Some(value) = pick(base.get(key), mine.get(key), theirs.get(key)) {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(provenance) = theirs.get("provenance") {
        out.insert("provenance".into(), provenance.clone());
    }
    out.insert(
        "layers_by_role".into(),
        merge_roles(
            &base["layers_by_role"],
            &mine["layers_by_role"],
            &theirs["layers_by_role"],
        ),
    );
    let macros = merge_list(
        &list(base.get("macros")),
        &list(mine.get("macros")),
        &list(theirs.get("macros")),
    );
    out.insert("macros".into(), Value::Array(macros));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn layer(id: &str, gain: f64, mode: &str) -> Value {
        json!({"id": id, "name": id, "params": {"gain": gain, "mode": mode}, "region": {"kind": "relative", "start": 0.0, "end": 1.0}})
    }

    fn scene(name: &str, layers: &[Value], hash: &str) -> Value {
        json!({
            "id": "s", "name": name, "tags": [], "provenance": {"hash": hash},
            "layers_by_role": {"strip.main": layers}, "macros": [],
        })
    }

    #[test]
    fn what_mine_left_takes_theirs_and_what_mine_changed_stays() {
        let base = scene("Demo", &[layer("1", 1.0, "fill")], "a");
        let mine = scene("Mine", &[layer("1", 2.0, "fill")], "a");
        let theirs = scene("Demo 2", &[layer("1", 1.0, "pulse")], "b");
        let merged = merge(&base, &mine, &theirs);
        assert_eq!(merged["name"], "Mine");
        let params = &merged["layers_by_role"]["strip.main"][0]["params"];
        assert_eq!(params["gain"], 2.0);
        assert_eq!(params["mode"], "pulse");
        assert_eq!(merged["provenance"]["hash"], "b");
        assert_eq!(merged["id"], "s");
    }

    #[test]
    fn added_removed_and_changed_layers() {
        let base = scene(
            "D",
            &[
                layer("1", 1.0, "fill"),
                layer("2", 1.0, "fill"),
                layer("3", 1.0, "fill"),
            ],
            "a",
        );
        // Mine removed 1, changed 3 and added 9
        let mine = scene(
            "D",
            &[
                layer("2", 1.0, "fill"),
                layer("3", 5.0, "fill"),
                layer("9", 1.0, "fill"),
            ],
            "a",
        );
        // Theirs removed 2 and 3 and added 4
        let theirs = scene(
            "D",
            &[layer("1", 1.0, "fill"), layer("4", 1.0, "fill")],
            "b",
        );
        let merged = merge(&base, &mine, &theirs);
        let ids: Vec<&str> = merged["layers_by_role"]["strip.main"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["id"].as_str().unwrap())
            .collect();
        // 1 stays removed, 2 goes (mine left it), 3 stays (mine changed it), 4 is added, 9 is kept
        assert_eq!(ids, ["4", "3", "9"]);
    }

    #[test]
    fn layers_follow_theirs_order_then_mines_own() {
        let base = scene(
            "D",
            &[layer("1", 1.0, "fill"), layer("2", 1.0, "fill")],
            "a",
        );
        let mine = scene(
            "D",
            &[
                layer("2", 1.0, "fill"),
                layer("1", 1.0, "fill"),
                layer("m", 1.0, "fill"),
            ],
            "a",
        );
        let theirs = scene(
            "D",
            &[layer("1", 1.0, "fill"), layer("2", 1.0, "fill")],
            "a",
        );
        let merged = merge(&base, &mine, &theirs);
        let ids: Vec<&str> = merged["layers_by_role"]["strip.main"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["1", "2", "m"]);
    }

    #[test]
    fn a_list_is_one_value() {
        let mut base = scene("D", &[layer("1", 1.0, "fill")], "a");
        base["layers_by_role"]["strip.main"][0]["params"]["palette"] = json!(["#000000"]);
        let mut mine = base.clone();
        mine["layers_by_role"]["strip.main"][0]["params"]["palette"] = json!(["#111111"]);
        let mut theirs = base.clone();
        theirs["layers_by_role"]["strip.main"][0]["params"]["palette"] =
            json!(["#222222", "#333333"]);
        theirs["tags"] = json!(["new"]);
        let merged = merge(&base, &mine, &theirs);
        assert_eq!(
            merged["layers_by_role"]["strip.main"][0]["params"]["palette"],
            json!(["#111111"])
        );
        assert_eq!(merged["tags"], json!(["new"]));
    }

    #[test]
    fn macros_merge_by_id() {
        let m = |id: &str, value: f64| json!({"id": id, "name": id, "value": value, "targets": []});
        let mut base = scene("D", &[], "a");
        base["macros"] = json!([m("a", 0.5)]);
        let mut mine = base.clone();
        mine["macros"] = json!([m("a", 0.9), m("mine", 0.1)]);
        let mut theirs = base.clone();
        theirs["macros"] = json!([m("a", 0.5), m("new", 0.2)]);
        let merged = merge(&base, &mine, &theirs);
        let macros = merged["macros"].as_array().unwrap();
        assert_eq!(macros.len(), 3);
        assert_eq!(macros[0]["value"], 0.9);
        assert_eq!(macros[1]["id"], "new");
        assert_eq!(macros[2]["id"], "mine");
    }
}
