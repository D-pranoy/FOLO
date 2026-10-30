//! The copy an apply writes (ADR-0038 D11): the item's scene with its layers
//! bound to the zone's surfaces and scaled to them, and its provenance.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::bind::{self, Surface, Wants};
use crate::scale::scale_layers;

/// The pack item an apply copies.
pub struct Source<'a> {
    pub pack: &'a str,
    pub version: &'a str,
    pub item_id: &'a str,
    /// The item's hash (D5), which provenance carries.
    pub hash: &'a str,
    pub item: &'a Value,
}

/// A copy and where its roles went.
#[derive(Debug, PartialEq)]
pub struct Copy {
    pub scene: Value,
    /// Scene role: id of the surface it went to.
    pub roles: BTreeMap<String, String>,
    pub skipped: Vec<String>,
}

/// The copy of `source` for a zone with `surfaces`, as the scene `scene_id`.
/// `min_elements` is what an effect needs of a surface, by effect id.
#[must_use]
pub fn build(
    source: &Source,
    scene_id: &str,
    surfaces: &[Surface],
    overrides: &BTreeMap<String, String>,
    min_elements: &dyn Fn(&str) -> u64,
) -> Copy {
    let scene = &source.item["scene"];
    let by_role = roles_of(scene);
    let needs = by_role
        .iter()
        .map(|(role, layers)| {
            let most = layers
                .iter()
                .map(|layer| min_elements(layer["effect"]["id"].as_str().unwrap_or_default()))
                .max()
                .unwrap_or(1);
            (role.clone(), most)
        })
        .collect();
    let fallbacks = string_map(&source.item["fallbacks"]);
    let led_count = source.item["led_count"].as_u64().unwrap_or(0);
    let binding = bind::bind(
        &Wants {
            roles: &needs,
            fallbacks: &fallbacks,
            led_count,
            overrides,
        },
        surfaces,
    );
    let mut layers_by_role = Map::new();
    let mut roles = BTreeMap::new();
    for (role, index) in &binding.bound {
        let surface = &surfaces[*index];
        let mut layers = by_role[role].clone();
        scale_layers(
            &mut layers,
            i64::try_from(led_count).unwrap_or(0),
            i64::try_from(surface.width).unwrap_or(0),
        );
        layers_by_role.insert(surface.role.clone(), Value::Array(layers));
        roles.insert(role.clone(), surface.id.clone());
    }
    let mut copy = Map::new();
    copy.insert("id".into(), scene_id.into());
    for key in ["name", "tags"] {
        if let Some(value) = scene.get(key) {
            copy.insert(key.into(), value.clone());
        }
    }
    copy.insert(
        "provenance".into(),
        json!({
            "pack": source.pack,
            "version": source.version,
            "item": source.item_id,
            "hash": source.hash,
        }),
    );
    copy.insert("layers_by_role".into(), Value::Object(layers_by_role));
    copy.insert("macros".into(), scene["macros"].clone());
    Copy {
        scene: Value::Object(copy),
        roles,
        skipped: binding.skipped,
    }
}

/// A scene's layers by role.
fn roles_of(scene: &Value) -> BTreeMap<String, Vec<Value>> {
    scene["layers_by_role"]
        .as_object()
        .map(|roles| {
            roles
                .iter()
                .map(|(role, layers)| {
                    (role.clone(), layers.as_array().cloned().unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(id: &str, role: &str, cells: u64) -> Surface {
        Surface {
            id: id.into(),
            canvas: false,
            role: role.into(),
            width: cells,
            cells,
        }
    }

    fn item() -> Value {
        json!({
            "id": "demo",
            "led_count": 100,
            "fallbacks": {},
            "scene": {
                "id": "demo",
                "name": "Demo",
                "tags": ["a"],
                "layers_by_role": {"strip.main": [
                    {"id": "1", "effect": {"id": "audio-visualizer"},
                     "region": {"kind": "cells", "start": 0, "end": 49}},
                    {"id": "2", "effect": {"id": "audio-visualizer"},
                     "region": {"kind": "relative", "start": 0.0, "end": 1.0}},
                ]},
                "macros": [],
            },
        })
    }

    fn source(item: &Value) -> Source<'_> {
        Source {
            pack: "a/b",
            version: "1.0.0",
            item_id: "demo",
            hash: "00",
            item,
        }
    }

    #[test]
    fn layers_go_under_the_surfaces_role_scaled_to_its_width() {
        let item = item();
        let surfaces = [strip("s1", "strip.left", 10)];
        let copy = build(&source(&item), "new", &surfaces, &BTreeMap::new(), &|_| 1);
        assert_eq!(copy.roles["strip.main"], "s1");
        let layers = &copy.scene["layers_by_role"]["strip.left"];
        assert_eq!(
            layers[0]["region"],
            json!({"kind": "cells", "start": 0, "end": 4})
        );
        assert_eq!(layers[1]["region"]["kind"], "relative");
        assert_eq!(copy.scene["id"], "new");
        assert_eq!(copy.scene["name"], "Demo");
        assert_eq!(
            copy.scene["provenance"],
            json!({"pack": "a/b", "version": "1.0.0", "item": "demo", "hash": "00"})
        );
    }

    #[test]
    fn a_role_no_surface_can_take_is_skipped_and_dropped() {
        let item = item();
        let surfaces = [strip("s1", "strip.left", 10)];
        let copy = build(&source(&item), "n", &surfaces, &BTreeMap::new(), &|_| 11);
        assert_eq!(copy.skipped, ["strip.main"]);
        assert_eq!(copy.scene["layers_by_role"], json!({}));
    }
}
