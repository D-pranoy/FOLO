//! Applying scenes and Reset all (RB-09 code change 4, ADR-0038 D11 to D16
//! and D21, docs/specs/client-api.md#applying-scenes-and-reset-all). The
//! pure parts (binding, scaling, merging) are `folo-library`'s; this reads
//! the documents, writes the show and keeps the bases in `applied/`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json, Response};
use folo_library::bind::Surface;
use folo_library::copy::{self, Source};
use folo_library::merge::merge;
use folo_library::reference;
use folo_protocol::clamp::{clamp, definition};
use folo_store::{Doc, files};
use serde_json::{Map, Value, json};

use crate::api::{Body, Refused, ZONES, check_revision, find, list, read_json, written};
use crate::node::Node;

type Shared = State<Arc<Node>>;

/// What Reset all applies when `default_scene` is empty or names nothing.
const FALLBACK_SCENE: &str = "folo/core:default";
/// The pack that holds a bare id the show doesn't know (ADR-0038 D2).
const MY_PRESETS: &str = "local/my-presets";
/// The black Reset all gives every surface, and the master it restores.
const RESET_BACKGROUND: &str = "#000000";
/// `show.json`'s limits when the schema can't be read.
const SCENE_LIMIT: usize = 256;
const BINDING_LIMIT: usize = 256;

/// Serialises the writes of `show.json` that also touch `applied/`, so a base
/// written for one write isn't pruned by another's older view of the show.
static SHOW_WRITES: Mutex<()> = Mutex::new(());

/// Held from before the show is written until its bases are written and pruned.
pub fn show_writes() -> MutexGuard<'static, ()> {
    SHOW_WRITES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What an effect needs of a surface, by effect id (`min_elements`).
static MIN_ELEMENTS: LazyLock<BTreeMap<String, u64>> = LazyLock::new(|| {
    folo_protocol::catalog::effects()
        .iter()
        .filter_map(|effect| {
            Some((
                effect["id"].as_str()?.to_owned(),
                effect["min_elements"].as_u64()?,
            ))
        })
        .collect()
});

fn min_elements(effect: &str) -> u64 {
    MIN_ELEMENTS.get(effect).copied().unwrap_or(1)
}

/// What a reference names.
enum Target {
    /// A scene of the show, which only becomes active.
    Show(String),
    /// A scene item of a pack, which is copied.
    Pack {
        pack: String,
        item: String,
        scene: folo_library::SceneItem,
    },
}

/// Reads a reference as D2 says: a pack item, an alias, a scene of the show,
/// then an item of `local/my-presets`. `None` when it names nothing.
fn resolve(node: &Node, show: &Value, text: &str) -> Option<Target> {
    let pack_item = |text: &str| {
        let (pack, item) = reference::parse(text)?;
        let scene = node.library.scene(pack, item).ok()?;
        Some(Target::Pack {
            pack: pack.to_owned(),
            item: item.to_owned(),
            scene,
        })
    };
    if reference::parse(text).is_some() {
        return pack_item(text);
    }
    if let Some(aliased) = node.library.alias(text) {
        return pack_item(&aliased);
    }
    if list_of(show, "scenes")
        .iter()
        .any(|scene| scene["id"] == text)
    {
        return Some(Target::Show(text.to_owned()));
    }
    pack_item(&format!("{MY_PRESETS}:{text}"))
}

fn list_of<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    document[key].as_array().map_or(&[], Vec::as_slice)
}

fn limit(show_key: &str, fallback: usize) -> usize {
    definition("ShowDocument")
        .and_then(|schema| schema["properties"][show_key]["maxItems"].as_u64())
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(fallback)
}

// ---------------------------------------------------------------- the bases

fn bases_dir(node: &Node) -> PathBuf {
    node.store.dir().join("applied")
}

fn read_base(node: &Node, scene_id: &str) -> Option<Value> {
    let bytes = fs::read(bases_dir(node).join(reference::base_file_name(scene_id))).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_base(node: &Node, scene: &Value) {
    let id = scene["id"].as_str().unwrap_or_default();
    let path = bases_dir(node).join(reference::base_file_name(id));
    let written = serde_json::to_vec_pretty(scene)
        .map_err(std::io::Error::other)
        .and_then(|bytes| files::write_atomic(&path, &bytes, false));
    if let Err(why) = written {
        // The apply stands; a later `keep_changes` on it answers `no_base`
        log::error!("applied: the base of scene {id} can't be written: {why}");
        // A stale base would be merged against as if it were current
        let _ = fs::remove_file(&path);
    }
}

/// Removes the bases of scenes that have left `show` or lost their provenance
/// (ADR-0038 D11).
pub fn prune_bases(node: &Node) {
    let show = node.store.get(Doc::Show);
    let show = &*show;
    let keep: BTreeSet<String> = list_of(show, "scenes")
        .iter()
        .filter(|scene| scene.get("provenance").is_some())
        .filter_map(|scene| scene["id"].as_str())
        .map(reference::base_file_name)
        .collect();
    let Ok(entries) = fs::read_dir(bases_dir(node)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Only files named as bases are ours to remove
        let is_base = name
            .strip_suffix(".json")
            .is_some_and(|stem| stem.len() == 64);
        if is_base
            && !keep.contains(&name)
            && let Err(why) = fs::remove_file(entry.path())
        {
            log::warn!("applied: {name} can't be removed: {why}");
        }
    }
}

/// Drops the bindings of zones that no longer exist (ADR-0038 D13).
pub fn drop_stale_bindings(show: &mut Value, zones: &Value) {
    let known: BTreeSet<&str> = list_of(zones, "zones")
        .iter()
        .filter_map(|zone| zone["id"].as_str())
        .collect();
    if let Some(bindings) = show.get_mut("bindings").and_then(Value::as_array_mut) {
        bindings.retain(|binding| {
            binding["zone"]
                .as_str()
                .is_some_and(|zone| known.contains(zone))
        });
    }
}

// ---------------------------------------------------------------- one apply

/// What the request asks of an apply of a pack item.
struct Ask<'a> {
    /// `zones.json`, for the zones that still exist.
    zones: &'a Value,
    roles: &'a BTreeMap<String, String>,
    keep_changes: bool,
}

/// A scene written, with the base to keep for it.
struct Written {
    result: Value,
    base: Option<Value>,
}

fn new_scene_id(show: &Value) -> String {
    loop {
        let id = folo_store::new_id();
        if !list_of(show, "scenes")
            .iter()
            .any(|scene| scene["id"] == id)
        {
            return id;
        }
    }
}

fn binding_index(show: &Value, zone: &str, scene: &str) -> Option<usize> {
    list_of(show, "bindings")
        .iter()
        .position(|b| b["zone"] == zone && b["scene"] == scene)
}

fn strings(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// The remembered overrides with the request's on top; `""` returns a role to automatic.
fn overrides_of(
    show: &Value,
    zone: &str,
    scene: &str,
    asked: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut roles = binding_index(show, zone, scene)
        .map(|at| strings(&show["bindings"][at]["roles"]))
        .unwrap_or_default();
    for (role, surface) in asked {
        if surface.is_empty() {
            roles.remove(role);
        } else {
            roles.insert(role.clone(), surface.clone());
        }
    }
    roles
}

/// Stores `roles` as the overrides of `scene` in `zone`: none left removes the entry.
fn remember(
    show: &mut Value,
    zone: &str,
    scene: &str,
    roles: &BTreeMap<String, String>,
) -> Result<(), Refused> {
    let at = binding_index(show, zone, scene);
    let bindings = show["bindings"].as_array().map_or(0, Vec::len);
    if roles.is_empty() {
        if let (Some(at), Some(list)) = (at, show["bindings"].as_array_mut()) {
            list.remove(at);
        }
        return Ok(());
    }
    let entry = json!({"zone": zone, "scene": scene, "roles": roles});
    match at {
        Some(at) => show["bindings"][at] = entry,
        None if bindings >= limit_of_bindings() => return Err(Refused::Full),
        None => match show["bindings"].as_array_mut() {
            Some(list) => list.push(entry),
            None => show["bindings"] = Value::Array(vec![entry]),
        },
    }
    Ok(())
}

fn limit_of_bindings() -> usize {
    limit("bindings", BINDING_LIMIT)
}

/// Applies `target` to `zone` in `show`, as ADR-0038 D11 to D15 say.
fn apply_to_zone(
    node: &Node,
    show: &mut Value,
    zone: &Value,
    target: &Target,
    ask: &Ask,
) -> Result<Written, Refused> {
    let zone_id = zone["id"].as_str().unwrap_or_default();
    match target {
        Target::Show(scene) => {
            if !ask.roles.is_empty() || ask.keep_changes {
                return Err(Refused::BadBody(
                    "a scene of the show takes no roles and no keep_changes".into(),
                ));
            }
            activate(show, zone_id, scene);
            Ok(Written {
                result: applied(zone_id, scene, &BTreeMap::new(), &[]),
                base: None,
            })
        }
        Target::Pack { pack, item, scene } => apply_item(
            node,
            show,
            zone,
            &format!("{pack}:{item}"),
            (pack, item, scene),
            ask,
        ),
    }
}

fn activate(show: &mut Value, zone: &str, scene: &str) {
    if !show["active"].is_object() {
        show["active"] = Value::Object(Map::new());
    }
    show["active"][zone] = Value::String(scene.to_owned());
}

fn applied(zone: &str, scene: &str, roles: &BTreeMap<String, String>, skipped: &[String]) -> Value {
    json!({"zone": zone, "scene": scene, "roles": roles, "skipped": skipped})
}

/// What an apply of a pack item replaces: the zone's active scene in place,
/// or nothing, for a new scene.
struct Slot {
    /// The id the copy is written under.
    id: String,
    /// Its position in `scenes` when written in place.
    at: Option<usize>,
    /// The zone's active scene as it is now.
    mine: Option<Value>,
}

fn slot_of(show: &Value, zones: &Value, zone: &str) -> Slot {
    let scenes = list_of(show, "scenes");
    let active = show["active"][zone].as_str().unwrap_or_default();
    let at = scenes.iter().position(|scene| scene["id"] == active);
    let elsewhere = show["active"].as_object().is_some_and(|map| {
        // A zone that was deleted no longer holds its scene
        map.iter().any(|(other, scene)| {
            other != zone && scene == active && find(zones, ZONES, other).is_some()
        })
    });
    match at {
        Some(at) if !elsewhere => Slot {
            id: active.to_owned(),
            at: Some(at),
            mine: Some(scenes[at].clone()),
        },
        Some(at) => Slot {
            id: new_scene_id(show),
            at: None,
            mine: Some(scenes[at].clone()),
        },
        None => Slot {
            id: new_scene_id(show),
            at: None,
            mine: None,
        },
    }
}

fn apply_item(
    node: &Node,
    show: &mut Value,
    zone: &Value,
    reference: &str,
    (pack, item, scene): (&str, &str, &folo_library::SceneItem),
    ask: &Ask,
) -> Result<Written, Refused> {
    let zone_id = zone["id"].as_str().unwrap_or_default();
    let surfaces = Surface::of_zone(zone);
    if let Some(unknown) = ask
        .roles
        .values()
        .find(|id| !id.is_empty() && !surfaces.iter().any(|s| s.id == **id))
    {
        return Err(Refused::BadBody(format!(
            "the zone has no surface {unknown}"
        )));
    }
    let slot = slot_of(show, ask.zones, zone_id);
    if slot.at.is_none() && list_of(show, "scenes").len() >= limit("scenes", SCENE_LIMIT) {
        return Err(Refused::Full);
    }
    let overrides = overrides_of(show, zone_id, reference, ask.roles);
    let source = Source {
        pack,
        version: &scene.version,
        item_id: item,
        hash: &scene.hash,
        item: &scene.item,
    };
    let copy = copy::build(&source, &slot.id, &surfaces, &overrides, &min_elements);
    // Nothing bound would write a scene with no layers over the zone's own
    if copy.roles.is_empty() {
        return Err(Refused::NoSurface {
            zone: zone_id.to_owned(),
            skipped: copy.skipped,
        });
    }
    let theirs = clamped_scene(&copy.scene)?;
    let written = if ask.keep_changes {
        merged(node, &slot, (pack, item), &theirs)?
    } else {
        theirs.clone()
    };
    store_scene(show, &slot, written);
    activate(show, zone_id, &slot.id);
    let remembered = binding_index(show, zone_id, reference)
        .map(|at| strings(&show["bindings"][at]["roles"]))
        .unwrap_or_default();
    if remembered != overrides {
        remember(show, zone_id, reference, &overrides)?;
    }
    Ok(Written {
        result: applied(zone_id, &slot.id, &copy.roles, &copy.skipped),
        base: Some(theirs),
    })
}

/// The copy as the show will hold it, so the base compares equal to a scene left alone.
fn clamped_scene(scene: &Value) -> Result<Value, Refused> {
    clamp("Scene", scene, &mut folo_store::new_id)
        .map_err(|why| Refused::BadBody(format!("the scene can't be clamped: {why}")))
}

/// D15: the merge of the zone's scene with the item, or `no_base`.
fn merged(
    node: &Node,
    slot: &Slot,
    (pack, item): (&str, &str),
    theirs: &Value,
) -> Result<Value, Refused> {
    let Some(mine) = slot
        .mine
        .as_ref()
        .filter(|scene| scene["provenance"]["pack"] == pack && scene["provenance"]["item"] == item)
    else {
        return Err(Refused::NoBase);
    };
    let mine_id = mine["id"].as_str().unwrap_or_default();
    let base = read_base(node, mine_id).ok_or(Refused::NoBase)?;
    let mut result = merge(&base, mine, theirs);
    result["id"] = Value::String(slot.id.clone());
    Ok(result)
}

fn store_scene(show: &mut Value, slot: &Slot, scene: Value) {
    match (slot.at, show["scenes"].as_array_mut()) {
        (Some(at), Some(scenes)) => scenes[at] = scene,
        (None, Some(scenes)) => scenes.push(scene),
        (_, None) => show["scenes"] = Value::Array(vec![scene]),
    }
}

// ---------------------------------------------------------------- the routes

/// The answer of an apply or a reset: the show as written and each zone's result.
fn answer(show: &Value, results: &[Value]) -> Response {
    Json(json!({"show": show, "results": results})).into_response()
}

fn object_of(body: Body) -> Result<Map<String, Value>, Refused> {
    if matches!(&body, Ok(bytes) if bytes.is_empty()) {
        return Ok(Map::new());
    }
    match read_json(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(Refused::BadBody("the body isn't a JSON object".into())),
        Err(refusal) => Err(Refused::Raw(refusal)),
    }
}

/// The request's fields, refusing others (`additionalProperties: false`).
fn only(map: &Map<String, Value>, keys: &[&str]) -> Result<(), Refused> {
    match map.keys().find(|key| !keys.contains(&key.as_str())) {
        Some(key) => Err(Refused::BadBody(format!("unknown field {key}"))),
        None => Ok(()),
    }
}

fn string_map(value: Option<&Value>) -> Result<BTreeMap<String, String>, Refused> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let bad = || Refused::BadBody("roles maps a role to a surface id".into());
    let map = value.as_object().ok_or_else(bad)?;
    // `ApplyResult.skipped` holds at most 64 roles, of at most 64 characters
    if map.len() > 64 || map.keys().any(|role| role.chars().count() > 64) {
        return Err(bad());
    }
    map.iter()
        .map(|(role, surface)| match surface.as_str() {
            Some(id) if id.chars().count() <= 64 => Ok((role.clone(), id.to_owned())),
            _ => Err(bad()),
        })
        .collect()
}

/// `POST /scenes/{ref}/apply`.
pub async fn apply(
    State(node): Shared,
    Path(reference): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    match apply_request(&node, &reference, &headers, body) {
        Ok(response) => response,
        Err(refused) => refused.response(),
    }
}

fn apply_request(
    node: &Arc<Node>,
    reference: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Refused> {
    let body = object_of(body)?;
    only(&body, &["zone", "roles", "keep_changes"])?;
    let _writing = show_writes();
    let zone_id = body
        .get("zone")
        .and_then(Value::as_str)
        .filter(|zone| zone.chars().count() <= 64)
        .ok_or_else(|| Refused::BadBody("zone is required".into()))?;
    let roles = string_map(body.get("roles"))?;
    let keep_changes = match body.get("keep_changes") {
        None => false,
        Some(Value::Bool(keep)) => *keep,
        Some(_) => return Err(Refused::BadBody("keep_changes is a boolean".into())),
    };
    let zones = node.store.get(Doc::Zones);
    let zone = find(&zones, ZONES, zone_id).ok_or(Refused::Absent("zone"))?;
    let mut results = Vec::new();
    let mut base = None;
    let stored = node.store.update(Doc::Show, |current| {
        check_revision(headers, current)?;
        let target = resolve(node, current, reference).ok_or(Refused::Absent("scene"))?;
        let mut show = current.clone();
        let done = apply_to_zone(
            node,
            &mut show,
            zone,
            &target,
            &Ask {
                zones: &zones,
                roles: &roles,
                keep_changes,
            },
        )?;
        drop_stale_bindings(&mut show, &zones);
        results.push(done.result);
        base = done.base;
        Ok::<_, Refused>(show)
    })?;
    finish(node, &base.into_iter().collect::<Vec<_>>());
    Ok(answer(&stored, &results))
}

/// After a show write: the engine, the new bases and the bases left over.
fn finish(node: &Node, bases: &[Value]) {
    written(node, Doc::Show);
    for scene in bases {
        write_base(node, scene);
    }
    prune_bases(node);
}

/// `POST /show/reset`.
pub async fn reset(State(node): Shared, headers: HeaderMap, body: Body) -> Response {
    match reset_request(&node, &headers, body) {
        Ok(response) => response,
        Err(refused) => refused.response(),
    }
}

fn reset_request(node: &Arc<Node>, headers: &HeaderMap, body: Body) -> Result<Response, Refused> {
    let body = object_of(body)?;
    only(&body, &["zones"])?;
    let _writing = show_writes();
    let asked = match body.get("zones") {
        None => Vec::new(),
        Some(Value::Array(list)) if list.len() <= 256 => list
            .iter()
            .map(|zone| zone.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| Refused::BadBody("zones lists zone ids".into()))?,
        Some(_) => return Err(Refused::BadBody("zones lists at most 256 zone ids".into())),
    };
    let zones = node.store.get(Doc::Zones);
    let ids: Vec<String> = if asked.is_empty() {
        list(&zones, ZONES)
            .iter()
            .filter_map(|zone| zone["id"].as_str().map(str::to_owned))
            .collect()
    } else {
        let mut seen = BTreeSet::new();
        asked
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect()
    };
    // A zone that doesn't exist refuses the whole reset, before anything changes
    if ids.iter().any(|id| find(&zones, ZONES, id).is_none()) {
        return Err(Refused::Absent("zone"));
    }
    let mut results = Vec::new();
    let mut bases = Vec::new();
    // The zones the default scene reached; the others are left as they are
    let mut covered: Vec<String> = Vec::new();
    let mut first_uncovered = None;
    let stored = node.store.update(Doc::Show, |current| {
        check_revision(headers, current)?;
        let mut show = current.clone();
        let target = default_target(node, &show).ok_or(Refused::Absent("scene"))?;
        let ask = Ask {
            zones: &zones,
            roles: &BTreeMap::new(),
            keep_changes: false,
        };
        for id in &ids {
            let zone = find(&zones, ZONES, id).ok_or(Refused::Absent("zone"))?;
            match apply_to_zone(node, &mut show, zone, &target, &ask) {
                Ok(done) => {
                    results.push(done.result);
                    bases.extend(done.base);
                    covered.push(id.clone());
                }
                Err(Refused::NoSurface {
                    zone: name,
                    skipped,
                }) => {
                    // Unchanged: its current scene, no roles bound, the roles skipped
                    let scene = show["active"][id.as_str()].as_str().unwrap_or_default();
                    results.push(applied(id, scene, &BTreeMap::new(), &skipped));
                    first_uncovered.get_or_insert((name, skipped));
                }
                Err(other) => return Err(other),
            }
        }
        if covered.is_empty()
            && let Some((zone, skipped)) = first_uncovered.take()
        {
            return Err(Refused::NoSurface { zone, skipped });
        }
        drop_stale_bindings(&mut show, &zones);
        Ok::<_, Refused>(show)
    })?;
    finish(node, &bases);
    restore_surfaces(node, &covered)?;
    Ok(answer(&stored, &results))
}

/// The default scene's target: `default_scene`, else (or when it names
/// nothing) `folo/core:default` (ADR-0038 D21).
fn default_target(node: &Node, show: &Value) -> Option<Target> {
    let named = show["default_scene"].as_str().unwrap_or_default();
    if !named.is_empty() {
        if let Some(target) = resolve(node, show, named) {
            return Some(target);
        }
        log::warn!("default_scene {named} names nothing: applying {FALLBACK_SCENE}");
    }
    let fallback = resolve(node, show, FALLBACK_SCENE);
    if fallback.is_none() {
        // The built-in packs are embedded, so this is a build without them
        log::error!("{FALLBACK_SCENE} isn't in the library");
    }
    fallback
}

/// Reset all's other half: the `master` of the zones' surfaces back to 1 and
/// their `background` to black, written to `zones.json` when one differs.
fn restore_surfaces(node: &Arc<Node>, ids: &[String]) -> Result<(), Refused> {
    // Numbers compare as written ("1" is not "1.0"), so read the master as a number
    let differs = |surface: &Value| {
        surface["master"].as_f64() != Some(1.0) || surface["background"] != RESET_BACKGROUND
    };
    let needed = node.store.get(Doc::Zones);
    let any = list(&needed, ZONES)
        .iter()
        .filter(|zone| ids.iter().any(|id| zone["id"] == *id))
        .any(|zone| list_of(zone, "surfaces").iter().any(differs));
    if !any {
        return Ok(());
    }
    node.store.update(Doc::Zones, |current| {
        let mut zones = current.clone();
        let list = zones["zones"].as_array_mut().into_iter().flatten();
        for zone in list.filter(|zone| ids.iter().any(|id| zone["id"] == *id)) {
            for surface in zone["surfaces"].as_array_mut().into_iter().flatten() {
                surface["master"] = json!(1.0);
                surface["background"] = json!(RESET_BACKGROUND);
            }
        }
        Ok::<_, Refused>(zones)
    })?;
    written(node, Doc::Zones);
    Ok(())
}
