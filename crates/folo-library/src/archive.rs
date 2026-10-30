//! `.folopack` archives: reading one with the checks of ADR-0038 D4, and
//! writing one deterministically (D9).

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::error::LibraryError;
use crate::limits;
use crate::names::{self, Entry, MANIFEST};

/// A pack's files by path, as bytes.
pub type Entries = BTreeMap<String, Vec<u8>>;

/// The archive's entries.
///
/// # Errors
///
/// [`LibraryError::Structural`] for anything D4's archive and entry-name
/// rules refuse.
pub fn read(bytes: &[u8]) -> Result<Entries, LibraryError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| LibraryError::structural("", format!("not a zip archive ({e})")))?;
    if archive.len() > limits::ARCHIVE_ENTRIES {
        return Err(LibraryError::structural(
            "",
            format!("more than {} entries", limits::ARCHIVE_ENTRIES),
        ));
    }
    let mut entries = Entries::new();
    let mut budget = limits::ARCHIVE_UNCOMPRESSED;
    for index in 0..archive.len() {
        let name = check_entry(&mut archive, index)?;
        let Some(name) = name else { continue };
        let limit = size_limit(&name)?.min(budget);
        let data = read_entry(&mut archive, index, &name, limit, budget)?;
        budget -= data.len() as u64;
        if entries.insert(name.clone(), data).is_some() {
            return Err(LibraryError::structural(name, "appears twice"));
        }
    }
    Ok(entries)
}

/// The name of entry `index` when it's a file to keep; `None` for a
/// directory entry.
fn check_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    index: usize,
) -> Result<Option<String>, LibraryError> {
    let file = archive
        .by_index_raw(index)
        .map_err(|e| LibraryError::structural("", format!("can't read entry {index} ({e})")))?;
    let name = file
        .name()
        .map_err(|e| LibraryError::structural("", format!("entry {index} has a bad name ({e})")))?
        .into_owned();
    if file.is_dir() {
        return Ok(None);
    }
    if file.encrypted() {
        return Err(LibraryError::structural(name, "is encrypted"));
    }
    if file.is_symlink() {
        return Err(LibraryError::structural(name, "is a link"));
    }
    if !matches!(
        file.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err(LibraryError::structural(
            name,
            "uses a method other than stored or deflate",
        ));
    }
    names::classify(&name).map_err(|reason| LibraryError::structural(name.clone(), reason))?;
    Ok(Some(name))
}

/// How big the entry may be: 1 MB for JSON files, 2 MB for previews.
fn size_limit(name: &str) -> Result<u64, LibraryError> {
    match names::classify(name).map_err(|r| LibraryError::structural(name, r))? {
        Entry::Preview => Ok(limits::PREVIEW_FILE),
        Entry::Manifest | Entry::Item(_) => Ok(limits::JSON_FILE),
    }
}

/// The entry's bytes, counted as they decompress: the sizes an archive
/// declares can be false.
fn read_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    index: usize,
    name: &str,
    limit: u64,
    budget: u64,
) -> Result<Vec<u8>, LibraryError> {
    let file = archive
        .by_index(index)
        .map_err(|e| LibraryError::structural(name, format!("can't be read ({e})")))?;
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| LibraryError::structural(name, format!("can't be read ({e})")))?;
    if data.len() as u64 > limit {
        let reason = if limit == budget {
            format!(
                "the archive holds more than {} bytes",
                limits::ARCHIVE_UNCOMPRESSED
            )
        } else {
            format!("is larger than {limit} bytes")
        };
        return Err(LibraryError::structural(name, reason));
    }
    Ok(data)
}

/// The archive of `entries`: `folo-pack.json` first, then the rest in byte
/// order of their paths, deflated, dated 1980-01-01 00:00, without extra
/// fields or directory entries.
///
/// # Errors
///
/// When the archive can't be written.
pub fn write(entries: &Entries) -> Result<Vec<u8>, LibraryError> {
    let epoch =
        DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).map_err(|e| LibraryError::Io {
            reason: e.to_string(),
        })?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(epoch)
        .unix_permissions(0o644);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let manifest = entries.get_key_value(MANIFEST);
    let rest = entries.iter().filter(|(path, _)| path.as_str() != MANIFEST);
    for (path, data) in manifest.into_iter().chain(rest) {
        let io = |e: &dyn std::fmt::Display| LibraryError::Io {
            reason: e.to_string(),
        };
        writer.start_file(path, options).map_err(|e| io(&e))?;
        writer.write_all(data).map_err(|e| io(&e))?;
    }
    let cursor = writer.finish().map_err(|e| LibraryError::Io {
        reason: e.to_string(),
    })?;
    Ok(cursor.into_inner())
}
