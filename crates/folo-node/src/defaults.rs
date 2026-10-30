//! Default surfaces, roles and mappings of a new zone (RB-08 step 5,
//! docs/architecture/spatial-model.md#defaults). They are filled in when a
//! zone is created through the API, only for what the client left out, and
//! never when a document is loaded or migrated.

use folo_supervisor::config::LOCAL_SOURCE;
use serde_json::{Value, json};

/// Makes a fresh id.
pub type NewId<'a> = &'a mut dyn FnMut() -> String;

/// The cells of the default canvases, wide by high.
const CANVAS_CELLS: (u64, u64) = (64, 36);
/// The most surfaces a zone holds (its schema's `maxItems`).
const MAX_SURFACES: usize = 64;
/// The most mappings a zone holds (its schema's `maxItems`).
/// The longest surface role (its schema's `maxLength`).
const MAX_ROLE: usize = 64;
const MAX_MAPPINGS: usize = 512;

/// Fills in `zone`, the clamped form of `sent`, with the defaults its `type`
/// calls for. `surfaces` is filled in only when `sent` has no such key, and
/// likewise `mappings`, so whatever the client sent stays as sent.
pub fn apply(sent: &Value, zone: &mut Value, new_id: NewId) {
    if omitted(sent, "surfaces") {
        zone["surfaces"] = Value::Array(surfaces(zone, new_id));
    }
    if omitted(sent, "mappings") {
        zone["mappings"] = Value::Array(mappings(zone, new_id));
    }
}

/// Whether the client left `key` out; a `null` is as good as nothing.
fn omitted(sent: &Value, key: &str) -> bool {
    sent.get(key).is_none_or(Value::is_null)
}

fn number(value: &Value, fallback: f64) -> f64 {
    value.as_f64().unwrap_or(fallback)
}

fn is_line(target: &Value) -> bool {
    target["kind"] == "line"
}

/// `strip.<target id>`; one that would pass `MAX_ROLE` keeps the start of the
/// id and ends in a hash of the whole id (FNV-1a), so it stays unique.
fn strip_role(target: &Value) -> String {
    let id = target["id"].as_str().unwrap_or_default();
    let role = format!("strip.{id}");
    if role.len() <= MAX_ROLE {
        return role;
    }
    let hash = id.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    let kept: String = id.chars().take(MAX_ROLE - "strip.".len() - 9).collect();
    format!("strip.{kept}-{hash:08x}")
}

/// A `canvas` surface of the default resolution on `plane`.
fn canvas(role: &str, plane: &Value, id: &str) -> Value {
    json!({
        "id": id,
        "kind": "canvas",
        "resolution": {"width": CANVAS_CELLS.0, "height": CANVAS_CELLS.1},
        "placement": {"kind": "plane", "plane": plane},
        "role": role,
        "sources": [LOCAL_SOURCE],
    })
}

/// `canvas.room` covers the floor: the origin is the corner at
/// `(−x/2, +y/2, 0)`, `u` runs along +x and `v` along −y.
fn room_plane(zone: &Value) -> Value {
    let x = number(&zone["bounds"]["x"], 4.0);
    let y = number(&zone["bounds"]["y"], 4.0);
    json!({
        "origin": {"x": -x / 2.0, "y": y / 2.0, "z": 0.0},
        "u": {"x": x, "y": 0.0, "z": 0.0},
        "v": {"x": 0.0, "y": -y, "z": 0.0},
    })
}

type Vec3 = [f64; 3];

/// Turns `v` by `yaw` about +z, after `pitch` about x, after `roll` about y
/// (the TV's own axes), in degrees: the matrix product `Rz · Rx · Ry`.
fn rotate(v: Vec3, [yaw, pitch, roll]: Vec3) -> Vec3 {
    let (sr, cr) = roll.to_radians().sin_cos();
    let v = [cr * v[0] + sr * v[2], v[1], -sr * v[0] + cr * v[2]];
    let (sp, cp) = pitch.to_radians().sin_cos();
    let v = [v[0], cp * v[1] - sp * v[2], sp * v[1] + cp * v[2]];
    let (sy, cy) = yaw.to_radians().sin_cos();
    [cy * v[0] - sy * v[1], sy * v[0] + cy * v[1], v[2]]
}

/// Millimetre precision keeps rounding noise (`-0`, `1e-17`) out of the file.
fn tidy(value: f64) -> f64 {
    let rounded = (value * 1e6).round() / 1e6;
    if rounded == 0.0 { 0.0 } else { rounded }
}

fn vec3(v: Vec3) -> Value {
    json!({"x": tidy(v[0]), "y": tidy(v[1]), "z": tidy(v[2])})
}

/// `canvas.screen` sits on the TV's plane: facing −y at orientation 0, the
/// origin at its top left as a viewer sees it, turned about its centre.
fn screen_plane(reference: &Value) -> Value {
    let (w, h) = (
        number(&reference["size"]["width"], 1.2),
        number(&reference["size"]["height"], 0.68),
    );
    let angles = |key: &str| number(&reference["orientation"][key], 0.0);
    let turn = [angles("yaw"), angles("pitch"), angles("roll")];
    let at = |key: &str| number(&reference["position"][key], 0.0);
    let centre = [at("x"), at("y"), at("z")];
    let corner = rotate([-w / 2.0, 0.0, h / 2.0], turn);
    json!({
        "origin": vec3([centre[0] + corner[0], centre[1] + corner[1], centre[2] + corner[2]]),
        "u": vec3(rotate([w, 0.0, 0.0], turn)),
        "v": vec3(rotate([0.0, 0.0, -h], turn)),
    })
}

fn has_tv(zone: &Value) -> bool {
    zone["type"] == "screen" && zone["reference"]["kind"] == "tv"
}

/// Every zone gets `canvas.room`, a screen zone with a TV also
/// `canvas.screen`, and each line target a `strip.<target>`. The strips
/// that would not fit under the schema's limit are left out.
fn surfaces(zone: &Value, new_id: NewId) -> Vec<Value> {
    let mut all = vec![canvas("canvas.room", &room_plane(zone), &new_id())];
    if has_tv(zone) {
        let plane = screen_plane(&zone["reference"]);
        all.push(canvas("canvas.screen", &plane, &new_id()));
    }
    for target in zone["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| is_line(t))
    {
        if all.len() >= MAX_SURFACES {
            break;
        }
        log_shortened(zone, target);
        all.push(json!({
            "id": new_id(),
            "kind": "strip",
            "resolution": {"width": target["geometry"]["count"].as_u64().unwrap_or(1), "height": 1},
            "placement": {"kind": "none"},
            "role": strip_role(target),
            "sources": [LOCAL_SOURCE],
        }));
    }
    all
}

fn log_shortened(zone: &Value, target: &Value) {
    let id = target["id"].as_str().unwrap_or_default();
    if format!("strip.{id}").len() > MAX_ROLE {
        let name = zone["name"].as_str().unwrap_or_default();
        log::warn!(
            "zone {name}: target {id}: the strip's role is shortened to fit {MAX_ROLE} characters"
        );
    }
}

fn surface_with_role<'a>(zone: &'a Value, role: &str) -> Option<&'a str> {
    zone["surfaces"]
        .as_array()?
        .iter()
        .find(|surface| surface["role"] == role)?["id"]
        .as_str()
}

/// One mapping for each target: 1:1 from its strip for a line, else a plane
/// mapping (area kernel, edge `clamp`) from the screen or else the room.
fn mappings(zone: &Value, new_id: NewId) -> Vec<Value> {
    let plane_role = if has_tv(zone) {
        "canvas.screen"
    } else {
        "canvas.room"
    };
    let mut all = Vec::new();
    for target in zone["targets"].as_array().into_iter().flatten() {
        let id = target["id"].clone();
        let mapping = if target["kind"] == "display" {
            None // a display has no positions to map onto
        } else if is_line(target) {
            let count = target["geometry"]["count"].as_u64().unwrap_or(1);
            surface_with_role(zone, &strip_role(target)).map(|surface| {
                json!({"surface": surface, "target": id, "method": "manual",
                    "ranges": [{"cells": {"start": 0, "count": count},
                        "elements": {"start": 0, "count": count}, "reversed": false}]})
            })
        } else {
            surface_with_role(zone, plane_role).map(|surface| {
                json!({"surface": surface, "target": id, "method": "plane",
                    "kernel": {"type": "area", "radius": 0.5}, "edge": "clamp"})
            })
        };
        if let Some(mut mapping) = mapping.filter(|_| all.len() < MAX_MAPPINGS) {
            mapping["id"] = Value::String(new_id());
            all.push(mapping);
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("id{next}")
        }
    }

    fn line(id: &str, count: u64) -> Value {
        json!({"id": id, "kind": "line", "geometry": {"kind": "line", "count": count}})
    }

    fn zone(kind: &str, reference: &str, targets: &[Value]) -> Value {
        json!({"type": kind, "bounds": {"x": 4.0, "y": 6.0, "z": 2.5},
            "reference": {"kind": reference, "position": {"x": 0.0, "y": 0.0, "z": 0.0},
                "orientation": {"yaw": 0.0, "pitch": 0.0, "roll": 0.0},
                "size": {"width": 1.2, "height": 0.68}},
            "targets": targets})
    }

    fn roles(zone: &Value) -> Vec<&str> {
        zone["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["role"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn every_type_but_a_tv_screen_gets_the_room_canvas_only() {
        for kind in ["music", "screen", "desk", "room", "custom"] {
            let mut zone = zone(kind, "none", &[]);
            apply(&json!({}), &mut zone, &mut ids());
            assert_eq!(roles(&zone), ["canvas.room"], "{kind}");
            assert_eq!(zone["surfaces"][0]["sources"], json!([LOCAL_SOURCE]));
        }
    }

    #[test]
    fn the_room_canvas_covers_the_floor_with_row_zero_on_the_far_side() {
        let mut zone = zone("room", "none", &[]);
        apply(&json!({}), &mut zone, &mut ids());
        let surface = &zone["surfaces"][0];
        assert_eq!(surface["resolution"], json!({"width": 64, "height": 36}));
        let plane = &surface["placement"]["plane"];
        assert_eq!(plane["origin"], json!({"x": -2.0, "y": 3.0, "z": 0.0}));
        assert_eq!(plane["u"], json!({"x": 4.0, "y": 0.0, "z": 0.0}));
        assert_eq!(plane["v"], json!({"x": 0.0, "y": -6.0, "z": 0.0}));
    }

    #[test]
    fn a_screen_zone_with_a_tv_gets_the_screen_canvas_on_the_tv_plane() {
        let mut zone = zone("screen", "tv", &[]);
        zone["reference"]["position"] = json!({"x": 1.0, "y": 2.0, "z": 0.8});
        apply(&json!({}), &mut zone, &mut ids());
        assert_eq!(roles(&zone), ["canvas.room", "canvas.screen"]);
        let plane = &zone["surfaces"][1]["placement"]["plane"];
        assert_eq!(plane["origin"], json!({"x": 0.4, "y": 2.0, "z": 1.14}));
        assert_eq!(plane["u"], json!({"x": 1.2, "y": 0.0, "z": 0.0}));
        assert_eq!(plane["v"], json!({"x": 0.0, "y": 0.0, "z": -0.68}));
    }

    #[test]
    fn a_tv_outside_a_screen_zone_and_a_screen_without_a_tv_get_no_screen_canvas() {
        for (kind, reference) in [("desk", "tv"), ("screen", "desk"), ("screen", "none")] {
            let mut zone = zone(kind, reference, &[]);
            apply(&json!({}), &mut zone, &mut ids());
            assert_eq!(roles(&zone), ["canvas.room"], "{kind} {reference}");
        }
    }

    #[test]
    fn the_orientation_turns_the_screen_about_its_centre() {
        let mut first = zone("screen", "tv", &[]);
        first["reference"]["orientation"]["yaw"] = json!(90.0);
        apply(&json!({}), &mut first, &mut ids());
        let plane = &first["surfaces"][1]["placement"]["plane"];
        assert_eq!(plane["u"], json!({"x": 0.0, "y": 1.2, "z": 0.0}));
        assert_eq!(plane["origin"], json!({"x": 0.0, "y": -0.6, "z": 0.34}));
        assert_eq!(
            rotate([1.0, 0.0, 0.0], [0.0, 90.0, 0.0]),
            [1.0, 0.0, 0.0].map(|v: f64| v)
        );
        for (angle, key, field, expected) in [
            (90.0, "pitch", "v", json!({"x": 0.0, "y": 0.68, "z": 0.0})),
            (90.0, "roll", "u", json!({"x": 0.0, "y": 0.0, "z": -1.2})),
        ] {
            let mut turned = zone("screen", "tv", &[]);
            turned["reference"]["orientation"][key] = json!(angle);
            apply(&json!({}), &mut turned, &mut ids());
            let plane = &turned["surfaces"][1]["placement"]["plane"];
            assert_eq!(plane[field], expected, "{key}");
        }
        let up = rotate([0.0, 1.0, 0.0], [0.0, 90.0, 0.0]);
        assert!(
            (up[2] - 1.0).abs() < 1e-9,
            "pitch turns +y towards +z: {up:?}"
        );
        let right = rotate([0.0, 0.0, 1.0], [0.0, 0.0, 90.0]);
        assert!(
            (right[0] - 1.0).abs() < 1e-9,
            "roll turns +z towards +x: {right:?}"
        );
    }

    #[test]
    fn a_line_target_gets_a_strip_and_a_one_to_one_mapping() {
        let mut zone = zone("music", "none", &[line("t1", 30)]);
        apply(&json!({}), &mut zone, &mut ids());
        assert_eq!(roles(&zone), ["canvas.room", "strip.t1"]);
        let strip = &zone["surfaces"][1];
        assert_eq!(strip["kind"], "strip");
        assert_eq!(strip["resolution"], json!({"width": 30, "height": 1}));
        assert_eq!(strip["placement"], json!({"kind": "none"}));
        let mapping = &zone["mappings"][0];
        assert_eq!(mapping["surface"], strip["id"]);
        assert_eq!(mapping["target"], "t1");
        assert_eq!(mapping["method"], "manual");
        assert_eq!(
            mapping["ranges"][0]["cells"],
            json!({"start": 0, "count": 30})
        );
        assert_eq!(
            mapping["ranges"][0]["elements"],
            json!({"start": 0, "count": 30})
        );
    }

    #[test]
    fn other_targets_map_from_the_room_or_from_the_screen() {
        let grid = json!({"id": "g", "kind": "grid"});
        let mut room = zone("room", "none", std::slice::from_ref(&grid));
        apply(&json!({}), &mut room, &mut ids());
        let mut tv = zone("screen", "tv", &[grid]);
        apply(&json!({}), &mut tv, &mut ids());
        for (zone, role) in [(&room, "canvas.room"), (&tv, "canvas.screen")] {
            let mapping = &zone["mappings"][0];
            assert_eq!(mapping["surface"], surface_with_role(zone, role).unwrap());
            assert_eq!(mapping["method"], "plane");
            assert_eq!(mapping["kernel"], json!({"type": "area", "radius": 0.5}));
            assert_eq!(mapping["edge"], "clamp");
        }
    }

    #[test]
    fn a_null_counts_as_left_out_and_a_display_gets_no_mapping() {
        let mut made = zone("room", "none", &[json!({"id": "d", "kind": "display"})]);
        apply(&json!({"surfaces": null}), &mut made, &mut ids());
        assert_eq!(roles(&made), ["canvas.room"]);
        assert_eq!(made["mappings"], json!([]));
    }

    #[test]
    fn a_long_role_is_shortened_deterministically_and_stays_unique() {
        let (a, b) = (
            format!("{}a", "x".repeat(70)),
            format!("{}b", "x".repeat(70)),
        );
        let mut made = zone("room", "none", &[line(&a, 5), line(&b, 5)]);
        apply(&json!({}), &mut made, &mut ids());
        let roles = roles(&made);
        assert!(
            roles[1..].iter().all(|role| role.len() <= MAX_ROLE),
            "{roles:?}"
        );
        assert_ne!(roles[1], roles[2]);
        assert_eq!(roles[1], strip_role(&line(&a, 5)));
        assert_eq!(made["mappings"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn what_the_client_sent_is_kept_even_when_empty() {
        let mut zone = zone("screen", "tv", &[line("t1", 30)]);
        zone["surfaces"] = json!([]);
        zone["mappings"] = json!([]);
        apply(
            &json!({"surfaces": [], "mappings": []}),
            &mut zone,
            &mut ids(),
        );
        assert_eq!(zone["surfaces"], json!([]));
        assert_eq!(zone["mappings"], json!([]));
    }

    #[test]
    fn only_the_missing_half_is_filled_in() {
        let mut zone = zone("room", "none", &[line("t1", 30)]);
        zone["surfaces"] = json!([{"id": "mine", "role": "strip.t1"}]);
        apply(&json!({"surfaces": [{}]}), &mut zone, &mut ids());
        assert_eq!(zone["surfaces"][0]["id"], "mine");
        assert_eq!(zone["mappings"][0]["surface"], "mine");
    }

    #[test]
    fn surfaces_stop_at_the_schema_limit_and_their_targets_get_no_mapping() {
        let targets: Vec<Value> = (0..70).map(|n| line(&format!("t{n}"), 1)).collect();
        let mut zone = zone("room", "none", &targets);
        apply(&json!({}), &mut zone, &mut ids());
        assert_eq!(zone["surfaces"].as_array().unwrap().len(), MAX_SURFACES);
        assert_eq!(zone["mappings"].as_array().unwrap().len(), MAX_SURFACES - 1);
    }
}
