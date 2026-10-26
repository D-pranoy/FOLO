//! Editing the items of a local pack (ADR-0038 D10): the pack's entries
//! after a write or a removal, ready to be checked as any pack is.

use serde_json::Value;

use crate::archive::Entries;
use crate::error::LibraryError;
use crate::library::MY_PRESETS;
use crate::limits;
use crate::names::{ItemType, MANIFEST};
use crate::pack::{Pack, file_hashes};

/// The entries of `pack` with the item `id` of `item_type` set to `body`.
///
/// # Errors
///
/// `bad_body`, `id_taken` or `list_full`, as `PUT …/items/{type}/{item}` says.
pub fn with_item(
    pack: &Pack,
    previews: &Entries,
    item_type: ItemType,
    id: &str,
    body: Value,
) -> Result<Entries, LibraryError> {
    let Value::Object(mut body) = body else {
        return Err(LibraryError::BadBody {
            reason: "an item is a JSON object".to_owned(),
        });
    };
    if id.is_empty() || id.chars().count() > limits::ITEM_ID {
        return Err(LibraryError::BadBody {
            reason: "an item id has 1 to 64 characters".to_owned(),
        });
    }
    if let Some(other) = pack
        .items
        .iter()
        .find(|i| i.id == id && i.item_type != item_type)
    {
        return Err(LibraryError::IdTaken {
            id: id.to_owned(),
            type_name: other.item_type.folder().to_owned(),
        });
    }
    let existing = pack.item(item_type, id);
    if existing.is_none() {
        check_room(pack, item_type)?;
    }
    body.insert("id".to_owned(), Value::String(id.to_owned()));
    let path = existing.map_or_else(|| free_path(pack, item_type), |i| i.path.clone());
    let mut entries = entries_of(pack, previews);
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(body)).expect("JSON serialises");
    bytes.push(b'\n');
    entries.insert(path, bytes);
    let mut manifest = pack.manifest.clone();
    if existing.is_none() {
        let list = &mut manifest["contents"][item_type.contents_key()];
        match list.as_array_mut() {
            Some(ids) => ids.push(Value::String(id.to_owned())),
            None => *list = Value::Array(vec![Value::String(id.to_owned())]),
        }
    }
    Ok(with_manifest(entries, manifest))
}

/// The entries of `pack` without the item `id` of `item_type`.
///
/// # Errors
///
/// `not_found` when the pack has no such item.
pub fn without_item(
    pack: &Pack,
    previews: &Entries,
    item_type: ItemType,
    id: &str,
) -> Result<Entries, LibraryError> {
    let item = pack
        .item(item_type, id)
        .ok_or_else(|| LibraryError::NotFound {
            what: format!("{} {} {id}", pack.id, item_type.folder()),
        })?;
    let mut entries = entries_of(pack, previews);
    entries.remove(&item.path);
    let mut manifest = pack.manifest.clone();
    if let Some(ids) = manifest["contents"][item_type.contents_key()].as_array_mut() {
        ids.retain(|v| v.as_str() != Some(id));
    }
    let file = manifest["previews"][id].as_str().map(str::to_owned);
    if let Some(map) = manifest["previews"].as_object_mut() {
        map.remove(id);
    }
    if let Some(file) = file {
        let still_used = manifest["previews"]
            .as_object()
            .is_some_and(|m| m.values().any(|v| v.as_str() == Some(file.as_str())));
        if !still_used {
            entries.remove(&format!("previews/{file}"));
        }
    }
    Ok(with_manifest(entries, manifest))
}

/// The pack's files and previews, without its manifest.
fn entries_of(pack: &Pack, previews: &Entries) -> Entries {
    let mut entries = pack.files.clone();
    entries.remove(MANIFEST);
    entries.extend(previews.iter().map(|(p, b)| (p.clone(), b.clone())));
    entries
}

/// `entries` with `manifest` as `folo-pack.json`, its `files` listing them.
fn with_manifest(mut entries: Entries, mut manifest: Value) -> Entries {
    manifest["files"] = file_hashes(entries.iter());
    let mut bytes = serde_json::to_vec_pretty(&manifest).expect("JSON serialises");
    bytes.push(b'\n');
    entries.insert(MANIFEST.to_owned(), bytes);
    entries
}

/// A new item needs room: 500 items, 60 scenes in `local/my-presets`.
fn check_room(pack: &Pack, item_type: ItemType) -> Result<(), LibraryError> {
    if pack.items.len() >= limits::PACK_ITEMS {
        return Err(LibraryError::ListFull {
            reason: format!("{} items", limits::PACK_ITEMS),
        });
    }
    let scenes = pack.of_type(ItemType::Scenes).count();
    if pack.id == MY_PRESETS && item_type == ItemType::Scenes && scenes >= limits::MY_PRESETS_SCENES
    {
        return Err(LibraryError::ListFull {
            reason: format!("{} scenes", limits::MY_PRESETS_SCENES),
        });
    }
    Ok(())
}

/// `<folder>/item-NNN.json` with the lowest free number.
fn free_path(pack: &Pack, item_type: ItemType) -> String {
    (1..=u32::MAX)
        .map(|n| format!("{}/item-{n:03}.json", item_type.folder()))
        .find(|path| !pack.files.contains_key(path))
        .expect("there are always more numbers")
}
