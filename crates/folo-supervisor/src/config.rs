//! The effect engine's configuration: the `config.apply` the node builds
//! from its documents (docs/architecture/control-plane.md#supervision).

use folo_protocol::clamp::clamp;
use serde_json::{Map, Value, json};

/// The local source's id in `sources.json`, as the migration names it.
pub const LOCAL_SOURCE: &str = "local";
/// The rate surfaces are published at (docs/architecture/overview.md).
const RATE_HZ: u32 = 60;
/// Canvases are published at most this size (docs/specs/source-link.md).
const CANVAS_MAX: (u64, u64) = (128, 72);

/// What live clients want from the local source besides its surfaces
/// (docs/specs/client-api.md#topics-and-delivery).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Wanted {
    /// The `beat` signal.
    pub beat: bool,
    /// The `spectrum` signal.
    pub spectrum: bool,
    /// The `levels` notifications of every surface.
    pub levels: bool,
}

/// `config.apply`'s params for the local source, without `revision`, which
/// the link adds when it sends them. Only zones whose `active` is not false
/// have surfaces assigned (ADR-0036 D6). Each signal keeps its index whatever
/// else is wanted (`beat` 0, `spectrum` 1), so a frame that arrives after
/// the wanted signals change is never taken for another signal's.
#[must_use]
pub fn local_config(zones: &Value, show: &Value, sources: &Value, wanted: Wanted) -> Value {
    let local = sources["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["id"] == LOCAL_SOURCE);
    let inputs = local.map_or_else(default_inputs, |s| s["inputs"].clone());
    let surface_ids: Vec<&str> = local
        .and_then(|s| s["surfaces"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();

    let mut assigned = Vec::new();
    let mut publish = Vec::new();
    let mut scene_ids: Vec<&str> = Vec::new();
    for id in surface_ids {
        let Some((zone, surface)) = find_surface(zones, id) else {
            continue;
        };
        let index = assigned.len();
        let scene = show["active"][zone["id"].as_str().unwrap_or_default()]
            .as_str()
            .unwrap_or_default();
        if !scene.is_empty() && !scene_ids.contains(&scene) {
            scene_ids.push(scene);
        }
        assigned.push(json!({
            "index": index,
            "surface": surface,
            "scene": scene,
            "role": surface["role"],
            "beat_offset_ms": zone["beat_offset_ms"],
            "album_colors": zone["album_colors"],
        }));
        let (width, height) = published_size(surface);
        publish.push(json!({
            "index": index,
            "width": width,
            "height": height,
            "rate_hz": RATE_HZ,
            "pixel_format": "rgb8",
            "lz4": false,
        }));
    }
    let signals: Vec<Value> = [("beat", wanted.beat), ("spectrum", wanted.spectrum)]
        .into_iter()
        .enumerate()
        .filter(|(_, (_, on))| *on)
        .map(|(index, (name, _))| json!({"name": name, "index": index}))
        .collect();
    let levels: Vec<usize> = if wanted.levels {
        (0..assigned.len()).collect()
    } else {
        Vec::new()
    };
    let scenes: Vec<Value> = scene_ids
        .iter()
        .filter_map(|id| {
            show["scenes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|s| s["id"] == *id)
                .cloned()
        })
        .collect();
    json!({
        "inputs": inputs,
        "surfaces": assigned,
        "scenes": scenes,
        "publish": {"surfaces": publish, "signals": signals, "levels": levels},
    })
}

fn default_inputs() -> Value {
    clamp("SourceInputs", &Value::Object(Map::new()), &mut String::new)
        .expect("an empty object clamps to the defaults")
}

/// The zone holding the surface `id`, and the surface. Only active zones
/// have surfaces to assign (ADR-0036 D6): `zones` holds the **effective**
/// `active` flags, which the node works out (`folo-node`'s `zones` module).
fn find_surface<'a>(zones: &'a Value, id: &str) -> Option<(&'a Value, &'a Value)> {
    zones["zones"].as_array()?.iter().find_map(|zone| {
        if zone["active"] == false {
            return None;
        }
        zone["surfaces"]
            .as_array()?
            .iter()
            .find(|s| s["id"] == id)
            .map(|surface| (zone, surface))
    })
}

fn published_size(surface: &Value) -> (u64, u64) {
    let width = surface["resolution"]["width"].as_u64().unwrap_or(1).max(1);
    let height = surface["resolution"]["height"].as_u64().unwrap_or(1).max(1);
    if surface["kind"] == "canvas" {
        (width.min(CANVAS_MAX.0), height.min(CANVAS_MAX.1))
    } else {
        (width, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clamped(definition: &str, value: &Value) -> Value {
        let mut count = 0;
        let mut ids = || {
            count += 1;
            format!("id{count}")
        };
        clamp(definition, value, &mut ids).unwrap()
    }

    fn documents() -> (Value, Value, Value) {
        let zones = clamped(
            "ZonesDocument",
            &json!({"zones": [
                {"id": "desk", "beat_offset_ms": 40, "album_colors": true, "surfaces": [
                    {"id": "wall", "kind": "canvas", "resolution": {"width": 256, "height": 144}, "role": "canvas.room"},
                ]},
                {"id": "default", "surfaces": [
                    {"id": "main", "kind": "strip", "resolution": {"width": 281, "height": 1}, "role": "strip.main"},
                ]},
            ]}),
        );
        let show = clamped(
            "ShowDocument",
            &json!({
                "active": {"default": "current", "desk": "current"},
                "scenes": [
                    {"id": "other", "name": "Other"},
                    {"id": "current", "name": "Current", "layers_by_role": {"strip.main": [{"id": "l1"}]}},
                ],
            }),
        );
        let sources = clamped(
            "SourcesDocument",
            &json!({"sources": [
                {"id": "local", "surfaces": ["main", "gone", "wall"], "inputs": {"audio": {"gain": 6}}},
            ]}),
        );
        (zones, show, sources)
    }

    #[test]
    fn inactive_zones_have_no_surfaces_assigned() {
        let (mut zones, show, sources) = documents();
        zones["zones"][0]["active"] = json!(false);
        let config = local_config(&zones, &show, &sources, Wanted::default());
        let surfaces = config["surfaces"].as_array().unwrap();
        assert_eq!(surfaces.len(), 1);
        // Surfaces are numbered in the order of the call
        assert_eq!(surfaces[0]["index"], 0);
        assert_eq!(surfaces[0]["surface"]["id"], "main");
        assert_eq!(config["publish"]["surfaces"].as_array().unwrap().len(), 1);
        let wanted = Wanted {
            levels: true,
            ..Wanted::default()
        };
        let config = local_config(&zones, &show, &sources, wanted);
        assert_eq!(config["publish"]["levels"], json!([0]));
    }

    #[test]
    fn assigned_surfaces_in_the_sources_order() {
        let (zones, show, sources) = documents();
        let config = local_config(&zones, &show, &sources, Wanted::default());
        let surfaces = config["surfaces"].as_array().unwrap();
        assert_eq!(surfaces.len(), 2);
        assert_eq!(surfaces[0]["index"], 0);
        assert_eq!(surfaces[0]["surface"]["id"], "main");
        assert_eq!(surfaces[0]["role"], "strip.main");
        assert_eq!(surfaces[0]["scene"], "current");
        assert_eq!(surfaces[0]["beat_offset_ms"], 0);
        assert_eq!(surfaces[1]["surface"]["id"], "wall");
        assert_eq!(surfaces[1]["beat_offset_ms"], 40);
        assert_eq!(surfaces[1]["album_colors"], true);
        assert!(same(&config["inputs"]["audio"]["gain"], &json!(6)));
        let scenes = config["scenes"].as_array().unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0]["id"], "current");
    }

    #[test]
    fn surfaces_are_published_at_60_hz_as_rgb8() {
        let (zones, show, sources) = documents();
        let publish = &local_config(&zones, &show, &sources, Wanted::default())["publish"];
        assert_eq!(
            publish["surfaces"],
            json!([
                {"index": 0, "width": 281, "height": 1, "rate_hz": 60, "pixel_format": "rgb8", "lz4": false},
                {"index": 1, "width": 128, "height": 72, "rate_hz": 60, "pixel_format": "rgb8", "lz4": false},
            ])
        );
        assert_eq!(publish["signals"], json!([]));
        assert_eq!(publish["levels"], json!([]));
    }

    #[test]
    fn signals_and_levels_live_clients_want() {
        let (zones, show, sources) = documents();
        let wanted = Wanted {
            beat: false,
            spectrum: true,
            levels: true,
        };
        let publish = &local_config(&zones, &show, &sources, wanted)["publish"];
        assert_eq!(
            publish["signals"],
            json!([{"name": "spectrum", "index": 1}])
        );
        assert_eq!(publish["levels"], json!([0, 1]));
        let all = Wanted {
            beat: true,
            ..wanted
        };
        let publish = &local_config(&zones, &show, &sources, all)["publish"];
        assert_eq!(
            publish["signals"],
            json!([{"name": "beat", "index": 0}, {"name": "spectrum", "index": 1}])
        );
    }

    #[test]
    fn without_a_local_source_the_defaults_and_nothing_to_render() {
        let (zones, show, _) = documents();
        let empty = clamped("SourcesDocument", &json!({}));
        let config = local_config(&zones, &show, &empty, Wanted::default());
        assert_eq!(config["surfaces"], json!([]));
        assert_eq!(config["inputs"]["audio"]["device"], "default");
    }

    #[test]
    fn the_params_are_valid_config_apply_params() {
        let (zones, show, sources) = documents();
        let wanted = Wanted {
            beat: true,
            spectrum: true,
            levels: true,
        };
        let mut params = local_config(&zones, &show, &sources, wanted);
        params["revision"] = json!(1);
        let again = clamped("ConfigApplyParams", &params);
        assert!(same(&again["surfaces"], &params["surfaces"]), "{again}");
        assert!(same(&again["publish"], &params["publish"]), "{again}");
    }

    /// Equal as JSON: numbers by value, since the clamper writes `number`
    /// fields as floats (`60.0` for `60`).
    fn same(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x
                .as_f64()
                .zip(y.as_f64())
                .is_some_and(|(x, y)| x.total_cmp(&y).is_eq()),
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
            }
            _ => a == b,
        }
    }

    #[test]
    fn numbers_compare_by_value() {
        assert!(same(&json!({"a": [60]}), &json!({"a": [60.0]})));
        assert!(!same(&json!({"a": [60]}), &json!({"a": [61]})));
        assert!(!same(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    }
}
