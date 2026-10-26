//! The library's folder: reading a pack's folder, and placing one by a
//! staging folder so a pack is never half written (ADR-0038 D1, D3, D8).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::archive::Entries;
use crate::error::LibraryError;
use crate::limits;
use crate::names::{self, Entry, ItemType, MANIFEST, PREVIEWS};

/// Where unfinished installs are written.
pub const STAGING: &str = ".staging";
/// Where a replaced pack waits until its replacement is in place.
pub const TRASH: &str = ".trash";
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The files of the pack folder `dir`, by path below it.
///
/// # Errors
///
/// [`LibraryError::Structural`] for an entry no pack may hold (a link, a
/// folder that isn't one of the pack's, a name that isn't allowed, a file
/// that is too large) and [`LibraryError::Io`] when a file can't be read.
pub fn read_folder(dir: &Path) -> Result<Entries, LibraryError> {
    let mut entries = Entries::new();
    for child in fs::read_dir(dir)? {
        let child = child?;
        let name = child.file_name().to_string_lossy().into_owned();
        let kind = child.file_type()?;
        if kind.is_symlink() {
            return Err(LibraryError::structural(name, "is a link"));
        }
        if kind.is_file() {
            read_file(&child.path(), &name, &mut entries)?;
        } else if name == PREVIEWS || ItemType::parse(&name).is_some() {
            for file in fs::read_dir(child.path())? {
                let file = file?;
                let path = format!("{name}/{}", file.file_name().to_string_lossy());
                if !file.file_type()?.is_file() {
                    return Err(LibraryError::structural(path, "isn't a file"));
                }
                read_file(&file.path(), &path, &mut entries)?;
            }
        } else {
            return Err(LibraryError::structural(name, "isn't part of a pack"));
        }
    }
    Ok(entries)
}

fn read_file(file: &Path, path: &str, entries: &mut Entries) -> Result<(), LibraryError> {
    let limit = match names::classify(path).map_err(|r| LibraryError::structural(path, r))? {
        Entry::Preview => limits::PREVIEW_FILE,
        Entry::Manifest | Entry::Item(_) => limits::JSON_FILE,
    };
    if fs::metadata(file)?.len() > limit {
        return Err(LibraryError::structural(
            path,
            format!("is larger than {limit} bytes"),
        ));
    }
    entries.insert(path.to_owned(), fs::read(file)?);
    Ok(())
}

/// A preview's bytes.
///
/// # Errors
///
/// When the file can't be read.
pub fn read_preview(dir: &Path, path: &str) -> Result<Vec<u8>, LibraryError> {
    Ok(fs::read(dir.join(path))?)
}

fn unique() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// The folder of pack `id` below `root`.
#[must_use]
pub fn folder(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

/// Writes `files` as the folder of pack `id`, replacing what was there: the
/// files go into a staging folder first, which then takes the place of the
/// old folder.
///
/// # Errors
///
/// [`LibraryError::Io`] when a file can't be written; the old folder stays.
pub fn place(root: &Path, id: &str, files: &Entries) -> Result<(), LibraryError> {
    let staged = root.join(STAGING).join(unique());
    let result = write_all(&staged, files).and_then(|()| swap(root, id, &staged));
    if result.is_err() {
        let _ = fs::remove_dir_all(&staged);
    }
    result
}

fn write_all(dir: &Path, files: &Entries) -> Result<(), LibraryError> {
    for (path, bytes) in files {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, bytes)?;
    }
    fs::create_dir_all(dir)?;
    Ok(())
}

/// Moves `staged` to the pack's place; the old folder waits in the trash
/// until then, and comes back if the move fails.
fn swap(root: &Path, id: &str, staged: &Path) -> Result<(), LibraryError> {
    let target = folder(root, id);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let trash = root
        .join(TRASH)
        .join(format!("{}~{}", id.replace('/', "~"), unique()));
    let had_old = target.exists();
    if had_old {
        fs::create_dir_all(root.join(TRASH))?;
        fs::rename(&target, &trash)?;
    }
    if let Err(error) = fs::rename(staged, &target) {
        if had_old {
            let _ = fs::rename(&trash, &target);
        }
        return Err(error.into());
    }
    if had_old {
        let _ = fs::remove_dir_all(&trash);
    }
    Ok(())
}

/// Removes the folder of pack `id`.
///
/// # Errors
///
/// [`LibraryError::Io`] when it can't be removed.
pub fn remove(root: &Path, id: &str) -> Result<(), LibraryError> {
    let target = folder(root, id);
    let trash = root
        .join(TRASH)
        .join(format!("{}~{}", id.replace('/', "~"), unique()));
    fs::create_dir_all(root.join(TRASH))?;
    fs::rename(&target, &trash)?;
    let _ = fs::remove_dir_all(&trash);
    Ok(())
}

/// After a crash: unfinished installs go, and a pack whose replacement
/// never arrived comes back from the trash.
pub fn recover(root: &Path) {
    let _ = fs::remove_dir_all(root.join(STAGING));
    let Ok(entries) = fs::read_dir(root.join(TRASH)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = name.split('~').take(2).collect::<Vec<_>>().join("/");
        let target = folder(root, &id);
        if !target.exists() && names::valid_pack_id(&id) {
            if let Some(parent) = target.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if fs::rename(entry.path(), &target).is_ok() {
                log::warn!("library: {id} came back from an interrupted replacement");
                continue;
            }
        }
        let _ = fs::remove_dir_all(entry.path());
    }
    let _ = fs::remove_dir(root.join(TRASH));
}

/// The pack folders below `root`, as `(id, path)`: `author/name` folders
/// whose names are valid. Hidden folders (the staging and the trash) and
/// files are not packs.
#[must_use]
pub fn pack_folders(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let Ok(authors) = fs::read_dir(root) else {
        return found;
    };
    for author in authors.flatten() {
        let author_name = author.file_name().to_string_lossy().into_owned();
        if author_name.starts_with('.') || !author.path().is_dir() {
            continue;
        }
        let Ok(packs) = fs::read_dir(author.path()) else {
            continue;
        };
        for pack in packs.flatten() {
            let id = format!("{author_name}/{}", pack.file_name().to_string_lossy());
            if pack.path().is_dir() {
                found.push((id, pack.path()));
            }
        }
    }
    found.sort();
    found
}

/// The manifest's path in a pack folder.
#[must_use]
pub fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(MANIFEST)
}
