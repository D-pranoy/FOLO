//! A pack read, validated, clamped and put into its canonical form
//! (ADR-0038 D3 to D7, docs/specs/pack-format.md#validation).

use std::collections::{BTreeMap, BTreeSet};

use folo_protocol::clamp::{ClampError, clamp};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::archive::{self, Entries};
use crate::canonical::{hex, item_hash, to_canonical};
use crate::error::LibraryError;
use crate::limits;
use crate::names::{self, Entry, ItemType, MANIFEST, PREVIEWS};

/// An item of a pack.
#[derive(Debug, Clone)]
pub struct Item {
    pub item_type: ItemType,
    /// The item's `id` field.
    pub id: String,
    /// Its file, such as `scenes/preset-001.json`.
    pub path: String,
    /// The item as clamped.
    pub value: Value,
    /// The SHA-256 of its canonical JSON.
    pub hash: String,
}

/// A pack in the library.
#[derive(Debug, Clone)]
pub struct Pack {
    pub id: String,
    pub builtin: bool,
    /// The manifest as clamped, its `files` listing the files below.
    pub manifest: Value,
    /// By type in `ItemType::ALL` order, each type in its `contents` order.
    pub items: Vec<Item>,
    /// `folo-pack.json` and the item files as the library keeps them:
    /// indented, with the manifest's `files` matching their bytes.
    pub files: Entries,
    /// Preview file (`previews/<name>`) and its size.
    pub previews: BTreeMap<String, u64>,
    /// The SHA-256 of the canonical archive.
    pub hash: String,
    /// What the pack takes of the library's 128 MB.
    pub bytes: u64,
}

/// A pack with the bytes of its previews, which it doesn't keep.
pub struct Assembled {
    pub pack: Pack,
    pub previews: Entries,
}

impl Pack {
    /// Whether it is a local pack, whose items can be written.
    #[must_use]
    pub fn editable(&self) -> bool {
        names::author(&self.id) == "local"
    }

    /// The pack's version.
    #[must_use]
    pub fn version(&self) -> &str {
        self.manifest["version"].as_str().unwrap_or_default()
    }

    /// An item by type and id.
    #[must_use]
    pub fn item(&self, item_type: ItemType, id: &str) -> Option<&Item> {
        self.items
            .iter()
            .find(|i| i.item_type == item_type && i.id == id)
    }

    /// The items of one type, in `contents` order.
    pub fn of_type(&self, item_type: ItemType) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |i| i.item_type == item_type)
    }

    /// The file name of an item's preview, or an empty string.
    #[must_use]
    pub fn preview_of(&self, id: &str) -> &str {
        self.manifest["previews"][id].as_str().unwrap_or_default()
    }
}

/// Reads `entries` as the pack `expected` (the folder's id, when it came
/// from a folder) or as an install.
///
/// # Errors
///
/// [`LibraryError::Structural`] for what ADR-0038 D4 lists.
pub fn assemble(
    entries: Entries,
    expected: Option<&str>,
    builtin: bool,
) -> Result<Assembled, LibraryError> {
    let raw = raw_manifest(&entries)?;
    let id = check_manifest(&raw, expected)?;
    check_files(&raw, &entries)?;
    let raw_items = raw_items(&entries)?;
    let order = check_contents(&raw, &raw_items)?;
    check_previews(&raw, &entries, &raw_items)?;
    let items = clamp_items(&id, &raw_items, &order)?;
    let manifest = clamp_manifest(&id, raw, &order)?;
    let previews: Entries = entries
        .into_iter()
        .filter(|(path, _)| path.starts_with("previews/"))
        .collect();
    finish(id, builtin, manifest, items, previews)
}

fn structural(path: &str, reason: impl Into<String>) -> LibraryError {
    LibraryError::structural(path, reason)
}

fn raw_manifest(entries: &Entries) -> Result<Map<String, Value>, LibraryError> {
    let bytes = entries
        .get(MANIFEST)
        .ok_or_else(|| structural(MANIFEST, "is missing"))?;
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(manifest)) => Ok(manifest),
        Ok(_) => Err(structural(MANIFEST, "isn't a JSON object")),
        Err(error) => Err(structural(MANIFEST, format!("isn't JSON ({error})"))),
    }
}

/// The required fields, the id and the version; the pack's id.
fn check_manifest(
    raw: &Map<String, Value>,
    expected: Option<&str>,
) -> Result<String, LibraryError> {
    for (key, string) in [
        ("id", true),
        ("version", true),
        ("folo", true),
        ("name", true),
        ("description", true),
        ("license", true),
        ("authors", false),
    ] {
        let ok = raw
            .get(key)
            .is_some_and(|v| if string { v.is_string() } else { v.is_array() });
        if !ok {
            let what = if string { "a string" } else { "a list" };
            return Err(structural(
                MANIFEST,
                format!("{key} is missing or isn't {what}"),
            ));
        }
    }
    for key in ["contents", "files"] {
        if !raw.get(key).is_some_and(Value::is_object) {
            return Err(structural(
                MANIFEST,
                format!("{key} is missing or isn't an object"),
            ));
        }
    }
    let id = raw["id"].as_str().unwrap_or_default();
    if !names::valid_pack_id(id) {
        return Err(structural(
            MANIFEST,
            "id isn't author/name in [a-z0-9-], 1 to 32 characters each",
        ));
    }
    let version = raw["version"].as_str().unwrap_or_default();
    if version.len() > 32 || semver::Version::parse(version).is_err() {
        return Err(structural(
            MANIFEST,
            "version isn't a semantic version of at most 32 characters",
        ));
    }
    if let Some(expected) = expected
        && expected != id
    {
        return Err(structural(
            MANIFEST,
            format!("id {id} isn't the folder's {expected}"),
        ));
    }
    Ok(id.to_owned())
}

/// `files` lists exactly the other entries, with their hashes.
fn check_files(raw: &Map<String, Value>, entries: &Entries) -> Result<(), LibraryError> {
    let listed = raw["files"].as_object().expect("checked");
    for path in entries.keys().filter(|p| p.as_str() != MANIFEST) {
        let Some(hash) = listed.get(path) else {
            return Err(structural(path, "isn't listed in files"));
        };
        let actual = hex(&Sha256::digest(&entries[path]));
        if hash.as_str() != Some(actual.as_str()) {
            return Err(structural(path, "doesn't match its hash in files"));
        }
    }
    if let Some(extra) = listed.keys().find(|p| !entries.contains_key(*p)) {
        return Err(structural(
            extra,
            "is listed in files but isn't in the pack",
        ));
    }
    Ok(())
}

/// An item file, parsed: (type, id, path, value).
type RawItem = (ItemType, String, String, Value);

fn raw_items(entries: &Entries) -> Result<Vec<RawItem>, LibraryError> {
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for (path, bytes) in entries {
        let Ok(Entry::Item(item_type)) = names::classify(path) else {
            continue;
        };
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| structural(path, format!("isn't JSON ({e})")))?;
        let id = match value.get("id") {
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            _ => return Err(structural(path, "has no id, or an empty one")),
        };
        if !value.is_object() {
            return Err(structural(path, "isn't a JSON object"));
        }
        if !seen.insert(id.clone()) {
            return Err(structural(
                path,
                format!("the id {id} is used twice in the pack"),
            ));
        }
        items.push((item_type, id, path.clone(), value));
    }
    if items.len() > limits::PACK_ITEMS {
        return Err(structural(
            "",
            format!("more than {} items", limits::PACK_ITEMS),
        ));
    }
    Ok(items)
}

/// `contents[type]` lists exactly the ids of the files of its folder; the
/// ids by type, in `contents` order.
fn check_contents(
    raw: &Map<String, Value>,
    items: &[RawItem],
) -> Result<Vec<(ItemType, Vec<String>)>, LibraryError> {
    let contents = raw["contents"].as_object().expect("checked");
    let mut order = Vec::new();
    for item_type in ItemType::ALL {
        let listed = match contents.get(item_type.contents_key()) {
            None => Vec::new(),
            Some(Value::Array(list)) => list
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    structural(
                        MANIFEST,
                        format!("contents.{} holds a non-string", item_type.contents_key()),
                    )
                })?,
            Some(_) => {
                return Err(structural(
                    MANIFEST,
                    format!("contents.{} isn't a list", item_type.contents_key()),
                ));
            }
        };
        let mut have: Vec<&str> = items
            .iter()
            .filter(|(t, ..)| *t == item_type)
            .map(|(_, id, ..)| id.as_str())
            .collect();
        let mut want: Vec<&str> = listed.iter().map(String::as_str).collect();
        have.sort_unstable();
        want.sort_unstable();
        if have != want {
            return Err(structural(
                MANIFEST,
                format!(
                    "contents.{} doesn't list exactly the ids of {}/",
                    item_type.contents_key(),
                    item_type.folder()
                ),
            ));
        }
        order.push((item_type, listed));
    }
    Ok(order)
}

/// Previews name items and files the pack has, and are the image they say.
fn check_previews(
    raw: &Map<String, Value>,
    entries: &Entries,
    items: &[RawItem],
) -> Result<(), LibraryError> {
    if let Some(named) = raw.get("previews") {
        let named = named
            .as_object()
            .ok_or_else(|| structural(MANIFEST, "previews isn't an object"))?;
        for (item, file) in named {
            if !items.iter().any(|(_, id, ..)| id == item) {
                return Err(structural(
                    MANIFEST,
                    format!("previews names the item {item}, which the pack doesn't have"),
                ));
            }
            let present = file
                .as_str()
                .is_some_and(|f| entries.contains_key(&format!("{PREVIEWS}/{f}")));
            if !present {
                return Err(structural(
                    MANIFEST,
                    format!("the preview of {item} isn't in the pack"),
                ));
            }
        }
    }
    for (path, bytes) in entries.iter().filter(|(p, _)| p.starts_with("previews/")) {
        if !is_image_of_extension(path, bytes) {
            return Err(structural(path, "isn't the image type its extension says"));
        }
    }
    Ok(())
}

/// The image type by content: PNG, JPEG, WebP or GIF.
fn is_image_of_extension(path: &str, bytes: &[u8]) -> bool {
    match path.rsplit('.').next() {
        Some("png") => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        Some("jpg") => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        Some("webp") => bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        Some("gif") => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        _ => false,
    }
}

/// An id the clamper makes: the first 8 hexadecimal characters of the
/// SHA-256 of `<pack id>:<item id>:<n>`, `n` counting from 1, so a pack
/// always clamps alike (ADR-0038 D5). The manifest has no item id.
fn id_maker<'a>(pack: &'a str, item: &'a str) -> impl FnMut() -> String + 'a {
    let mut n = 0u32;
    move || {
        n += 1;
        let digest = Sha256::digest(format!("{pack}:{item}:{n}").as_bytes());
        hex(&digest)[..8].to_owned()
    }
}

fn clamp_error(path: &str, error: &ClampError) -> LibraryError {
    structural(path, error.to_string())
}

fn clamp_items(
    pack: &str,
    raw: &[RawItem],
    order: &[(ItemType, Vec<String>)],
) -> Result<Vec<Item>, LibraryError> {
    let mut items = Vec::new();
    for (item_type, ids) in order {
        for id in ids {
            let (_, _, path, value) = raw
                .iter()
                .find(|(t, i, ..)| t == item_type && i == id)
                .expect("contents were checked against the files");
            let mut new_id = id_maker(pack, id);
            let clamped = clamp(item_type.definition(), value, &mut new_id)
                .map_err(|e| clamp_error(path, &e))?;
            if clamped["id"].as_str() != Some(id) {
                return Err(structural(path, "the id is longer than 64 characters"));
            }
            let hash = item_hash(&clamped).map_err(|e| structural(path, e.to_string()))?;
            items.push(Item {
                item_type: *item_type,
                id: id.clone(),
                path: path.clone(),
                value: clamped,
                hash,
            });
        }
    }
    Ok(items)
}

fn clamp_manifest(
    pack: &str,
    raw: Map<String, Value>,
    order: &[(ItemType, Vec<String>)],
) -> Result<Value, LibraryError> {
    let mut new_id = id_maker(pack, "");
    let mut manifest = clamp("PackManifest", &Value::Object(raw), &mut new_id)
        .map_err(|e| clamp_error(MANIFEST, &e))?;
    let contents: Map<String, Value> = order
        .iter()
        .map(|(t, ids)| {
            let ids = ids.iter().cloned().map(Value::String).collect();
            (t.contents_key().to_owned(), Value::Array(ids))
        })
        .collect();
    manifest["contents"] = Value::Object(contents);
    Ok(manifest)
}

/// An item as the library writes it: indented, with a final newline.
fn disk_form(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("a JSON value serialises");
    bytes.push(b'\n');
    bytes
}

/// The manifest's `files`: each path with the SHA-256 of its bytes.
pub fn file_hashes<'a>(files: impl Iterator<Item = (&'a String, &'a Vec<u8>)>) -> Value {
    let map: Map<String, Value> = files
        .map(|(path, bytes)| (path.clone(), Value::String(hex(&Sha256::digest(bytes)))))
        .collect();
    Value::Object(map)
}

/// The pack's files in the library's form, its hash, and its size.
fn finish(
    id: String,
    builtin: bool,
    mut manifest: Value,
    items: Vec<Item>,
    previews: Entries,
) -> Result<Assembled, LibraryError> {
    let mut files: Entries = items
        .iter()
        .map(|i| (i.path.clone(), disk_form(&i.value)))
        .collect();
    manifest["files"] = file_hashes(files.iter().chain(previews.iter()));
    files.insert(MANIFEST.to_owned(), disk_form(&manifest));
    let sizes: BTreeMap<String, u64> = previews
        .iter()
        .map(|(p, b)| (p.clone(), b.len() as u64))
        .collect();
    let bytes = files.values().map(|b| b.len() as u64).sum::<u64>() + sizes.values().sum::<u64>();
    let mut pack = Pack {
        id,
        builtin,
        manifest,
        items,
        files,
        previews: sizes,
        hash: String::new(),
        bytes,
    };
    let archive = archive::write(&canonical_entries(&pack, &previews)?)?;
    pack.hash = hex(&Sha256::digest(&archive));
    Ok(Assembled { pack, previews })
}

/// The entries of the pack's canonical archive: each item as its canonical
/// JSON and the manifest's `files` rewritten to match (ADR-0038 D9).
///
/// # Errors
///
/// When a value has no canonical form.
pub fn canonical_entries(pack: &Pack, previews: &Entries) -> Result<Entries, LibraryError> {
    let mut entries = Entries::new();
    for item in &pack.items {
        let text = to_canonical(&item.value).map_err(|e| structural(&item.path, e.to_string()))?;
        entries.insert(item.path.clone(), text.into_bytes());
    }
    entries.extend(previews.iter().map(|(p, b)| (p.clone(), b.clone())));
    let mut manifest = pack.manifest.clone();
    manifest["files"] = file_hashes(entries.iter());
    let text = to_canonical(&manifest).map_err(|e| structural(MANIFEST, e.to_string()))?;
    entries.insert(MANIFEST.to_owned(), text.into_bytes());
    Ok(entries)
}
