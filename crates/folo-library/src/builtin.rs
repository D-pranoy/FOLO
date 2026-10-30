//! The built-in packs and the alias table, embedded from `packs/` at build
//! time (ADR-0038 D1, D19).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::archive::Entries;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/builtin_files.rs"));
}

/// The alias table's path below `packs/`.
const ALIASES: &str = "aliases.json";

/// The files of each built-in pack, by pack id (`folo/core`).
#[must_use]
pub fn packs() -> BTreeMap<String, Entries> {
    let mut packs: BTreeMap<String, Entries> = BTreeMap::new();
    for (path, bytes) in embedded::FILES {
        let mut parts = path.splitn(3, '/');
        if let (Some(author), Some(name), Some(rest)) = (parts.next(), parts.next(), parts.next()) {
            packs
                .entry(format!("{author}/{name}"))
                .or_default()
                .insert(rest.to_owned(), bytes.to_vec());
        }
    }
    packs
}

/// The studio's built-in ids (`fx:*`, `genre:*`) and their references.
#[must_use]
pub fn aliases() -> BTreeMap<String, String> {
    let table = embedded::FILES
        .iter()
        .find(|(path, _)| *path == ALIASES)
        .and_then(|(_, bytes)| serde_json::from_slice::<Value>(bytes).ok());
    table
        .as_ref()
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(id, reference)| Some((id.clone(), reference.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}
