//! Backup archives (RB-09 step 7, ADR-0038 D25 to D27): writing one, and
//! reading one with every check made before anything changes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use folo_protocol::clamp::SCHEMA_VERSION;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::archive::{self, ArchiveLimits, Entries};
use crate::canonical::hex;
use crate::disk;
use crate::error::LibraryError;
use crate::limits;
use crate::names;
use crate::pack::{self, Assembled};

/// The manifest's name.
pub const MANIFEST: &str = "folo-backup.json";
/// The manifest's `format`.
pub const FORMAT: i64 = 1;
/// The documents a backup holds, in the order of the API.
pub const DOCUMENTS: [&str; 4] = ["zones.json", "show.json", "providers.json", "sources.json"];
/// Where the packs are, in the archive and beside the documents.
pub const LIBRARY: &str = "library";
/// Where the bases of applied scenes are.
pub const APPLIED: &str = "applied";

const BACKUP_LIMITS: ArchiveLimits = ArchiveLimits {
    entries: limits::BACKUP_ENTRIES,
    uncompressed: limits::BACKUP_UNCOMPRESSED,
};

/// What a backup is made of.
pub struct Contents {
    /// The four documents, as the node writes them, by file name.
    pub documents: Entries,
    /// Each installed and local pack's canonical entries, by pack id.
    pub packs: BTreeMap<String, Entries>,
    /// The bases, by file name.
    pub bases: Entries,
}

/// A backup that passed every check but the documents' own: nothing has
/// been changed yet.
pub struct Restored {
    /// The documents' bytes, by file name.
    pub documents: Entries,
    pub packs: Vec<Assembled>,
    /// The bases, by file name.
    pub bases: Entries,
}

/// The archive of `contents`, with its manifest first.
///
/// # Errors
///
/// When the archive can't be written.
pub fn write(
    contents: Contents,
    node_version: &str,
    created: u64,
) -> Result<Vec<u8>, LibraryError> {
    let mut entries = contents.documents;
    for (id, files) in contents.packs {
        entries.extend(
            files
                .into_iter()
                .map(|(path, bytes)| (format!("{LIBRARY}/{id}/{path}"), bytes)),
        );
    }
    entries.extend(
        contents
            .bases
            .into_iter()
            .map(|(name, bytes)| (format!("{APPLIED}/{name}"), bytes)),
    );
    // A backup this node couldn't restore is refused when it is made
    let total: u64 = entries.values().map(|b| b.len() as u64).sum();
    if entries.len() >= limits::BACKUP_ENTRIES || total > limits::BACKUP_UNCOMPRESSED {
        return Err(LibraryError::LibraryFull {
            reason: "the backup would be larger than a restore accepts".to_owned(),
        });
    }
    let files = pack::file_hashes(entries.iter());
    let manifest = json!({
        "format": FORMAT,
        "created": created,
        "node_version": node_version,
        "schema_version": SCHEMA_VERSION,
        "files": files,
    });
    let mut text = serde_json::to_vec_pretty(&manifest).map_err(|e| LibraryError::Io {
        reason: e.to_string(),
    })?;
    text.push(b'\n');
    entries.insert(MANIFEST.to_owned(), text);
    archive::write_first(&entries, MANIFEST)
}

/// Whether `name` is a base's file: 64 lowercase hexadecimal characters and `.json`.
#[must_use]
pub fn is_base_name(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(|stem| {
        stem.len() == 64
            && stem
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// How big the entry may be, or why a backup holds no such path (ADR-0038 D27).
fn entry_limit(path: &str) -> Result<u64, String> {
    if path == MANIFEST {
        return Ok(limits::JSON_FILE);
    }
    if DOCUMENTS.contains(&path) {
        return Ok(limits::DOCUMENT_FILE);
    }
    if let Some(name) = path.strip_prefix("applied/") {
        return if is_base_name(name) {
            Ok(limits::JSON_FILE)
        } else {
            Err("bases are 64 hexadecimal characters and .json".to_owned())
        };
    }
    let Some(rest) = path.strip_prefix("library/") else {
        return Err(
            "a backup holds folo-backup.json, the four documents, library/ and applied/".to_owned(),
        );
    };
    let (id, entry) = split_pack(rest).ok_or("packs are library/<author>/<name>/<entry>")?;
    if names::author(&id) == "folo" {
        return Err("the built-in packs aren't part of a backup".to_owned());
    }
    archive::pack_entry_limit(entry)
}

/// `author/name/entry` as the pack's id and the entry.
fn split_pack(rest: &str) -> Option<(String, &str)> {
    let mut parts = rest.splitn(3, '/');
    let (author, name, entry) = (parts.next()?, parts.next()?, parts.next()?);
    let id = format!("{author}/{name}");
    names::valid_pack_id(&id).then_some((id, entry))
}

/// The backup in `bytes`, checked as a whole: the archive, the paths, the
/// manifest and its hashes, and every pack (ADR-0038 D27). The documents
/// are returned as bytes for the caller to clamp.
///
/// # Errors
///
/// [`LibraryError::Structural`], or [`LibraryError::IncompatibleBackup`] for
/// a newer `format` or `schema_version`.
pub fn read(bytes: &[u8]) -> Result<Restored, LibraryError> {
    let mut entries = archive::read_with(bytes, BACKUP_LIMITS, &entry_limit)?;
    let manifest = entries
        .remove(MANIFEST)
        .ok_or_else(|| LibraryError::structural(MANIFEST, "is missing"))?;
    check_manifest(&manifest, &entries)?;
    let mut documents = Entries::new();
    for name in DOCUMENTS {
        let bytes = entries
            .remove(name)
            .ok_or_else(|| LibraryError::structural(name, "is missing"))?;
        documents.insert(name.to_owned(), bytes);
    }
    let mut bases = Entries::new();
    let mut packs: BTreeMap<String, Entries> = BTreeMap::new();
    for (path, bytes) in entries {
        if let Some(name) = path.strip_prefix("applied/") {
            bases.insert(name.to_owned(), bytes);
        } else if let Some((id, entry)) = path.strip_prefix("library/").and_then(split_pack) {
            packs.entry(id).or_default().insert(entry.to_owned(), bytes);
        }
    }
    Ok(Restored {
        documents,
        packs: assemble_all(packs)?,
        bases,
    })
}

/// Every pack as an install checks it, in the folder its id names.
fn assemble_all(packs: BTreeMap<String, Entries>) -> Result<Vec<Assembled>, LibraryError> {
    let mut assembled = Vec::with_capacity(packs.len());
    let mut bytes = 0u64;
    for (id, files) in packs {
        let pack = pack::assemble(files, Some(&id), false).map_err(|error| match error {
            LibraryError::Structural { path, reason } => {
                let inside = if path.is_empty() {
                    String::new()
                } else {
                    format!("/{path}")
                };
                LibraryError::structural(format!("{LIBRARY}/{id}{inside}"), reason)
            }
            other => other,
        })?;
        bytes += pack.pack.bytes;
        assembled.push(pack);
    }
    if assembled.len() > limits::LIBRARY_PACKS {
        return Err(LibraryError::structural(
            LIBRARY,
            format!("more than {} packs", limits::LIBRARY_PACKS),
        ));
    }
    if bytes > limits::LIBRARY_BYTES {
        return Err(LibraryError::structural(
            LIBRARY,
            format!("more than {} bytes of packs", limits::LIBRARY_BYTES),
        ));
    }
    Ok(assembled)
}

fn manifest_object(manifest: &[u8]) -> Result<Map<String, Value>, LibraryError> {
    match serde_json::from_slice::<Value>(manifest) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(LibraryError::structural(MANIFEST, "isn't a JSON object")),
        Err(error) => Err(LibraryError::structural(
            MANIFEST,
            format!("isn't JSON ({error})"),
        )),
    }
}

/// The manifest's version fields, then its shape, then `files` against the
/// other entries.
fn check_manifest(manifest: &[u8], entries: &Entries) -> Result<(), LibraryError> {
    let object = manifest_object(manifest)?;
    let whole = |key: &str, minimum: i64| {
        object
            .get(key)
            .and_then(Value::as_i64)
            .filter(|n| *n >= minimum)
            .ok_or_else(|| {
                LibraryError::structural(
                    MANIFEST,
                    format!("{key} is missing or isn't a whole number from {minimum}"),
                )
            })
    };
    // A newer format may shape the rest differently, so it's judged first
    let format = whole("format", 1)?;
    if format > FORMAT {
        return Err(LibraryError::IncompatibleBackup {
            what: "format",
            found: format,
            supported: FORMAT,
        });
    }
    let schema = whole("schema_version", 2)?;
    if schema > SCHEMA_VERSION {
        return Err(LibraryError::IncompatibleBackup {
            what: "schema_version",
            found: schema,
            supported: SCHEMA_VERSION,
        });
    }
    whole("created", 0)?;
    let version_ok = object
        .get("node_version")
        .and_then(Value::as_str)
        .is_some_and(|v| v.len() <= 32);
    if !version_ok {
        return Err(LibraryError::structural(
            MANIFEST,
            "node_version is missing or isn't a string of at most 32 characters",
        ));
    }
    if let Some(key) = object.keys().find(|k| {
        ![
            "format",
            "created",
            "node_version",
            "schema_version",
            "files",
        ]
        .contains(&k.as_str())
    }) {
        return Err(LibraryError::structural(
            MANIFEST,
            format!("{key} isn't a field of a backup"),
        ));
    }
    check_files(&object, entries)
}

/// `files` lists exactly the other entries, with their hashes.
fn check_files(object: &Map<String, Value>, entries: &Entries) -> Result<(), LibraryError> {
    let listed = object
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(|| LibraryError::structural(MANIFEST, "files is missing or isn't an object"))?;
    for (path, bytes) in entries {
        let Some(hash) = listed.get(path) else {
            return Err(LibraryError::structural(path, "isn't listed in files"));
        };
        if hash.as_str() != Some(hex(&Sha256::digest(bytes)).as_str()) {
            return Err(LibraryError::structural(
                path,
                "doesn't match its hash in files",
            ));
        }
    }
    let present: BTreeSet<&String> = entries.keys().collect();
    if let Some(missing) = listed.keys().find(|p| !present.contains(p)) {
        return Err(LibraryError::structural(
            missing,
            "is listed in files but isn't in the archive",
        ));
    }
    Ok(())
}

/// Writes the packs and the bases to `library_dir` and `applied_dir` for a
/// restore to move into place.
///
/// # Errors
///
/// [`LibraryError::Io`] when a file can't be written.
pub fn stage(
    library_dir: &Path,
    applied_dir: &Path,
    restored: &Restored,
) -> Result<(), LibraryError> {
    disk::write_all(library_dir, &Entries::new())?;
    for assembled in &restored.packs {
        let mut files = assembled.pack.files.clone();
        files.extend(assembled.previews.clone());
        disk::write_all(&library_dir.join(&assembled.pack.id), &files)?;
    }
    disk::write_all(applied_dir, &restored.bases)
}
