//! Backup and restore (RB-09 step 7, ADR-0038 D25 to D27): the archive of
//! the documents, the library and the bases, and the replacement of all
//! three through `restore/` and its marker.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use folo_library::archive::Entries;
use folo_library::backup::{self, APPLIED, Contents, DOCUMENTS, LIBRARY, Restored};
use folo_library::{LibraryError, limits};
use folo_protocol::clamp::SCHEMA_VERSION;
use folo_store::{Doc, files};
use serde_json::Value;

use crate::apply_api;
use crate::node::Node;

/// Where a restore is written before anything is replaced.
pub const RESTORE_DIR: &str = "restore";
/// Present from when `restore/` is complete until every move is done.
pub const MARKER: &str = "restore.pending";

/// One restore at a time: the heavy reading and staging happen before any
/// other lock, so concurrent restores would multiply their memory.
static RESTORES: Mutex<()> = Mutex::new(());

/// The four documents a backup holds.
const BACKUP_DOCS: [Doc; 4] = [Doc::Zones, Doc::Show, Doc::Providers, Doc::Sources];

/// A backup of the node as it is: the archive's bytes and its file name.
///
/// # Errors
///
/// [`LibraryError::Io`] when a pack's preview or a base can't be read.
pub fn backup(node: &Node) -> Result<(String, Vec<u8>), LibraryError> {
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // No apply writes a base while the bases are read
    let _writes = apply_api::show_writes();
    let documents: Entries = BACKUP_DOCS
        .iter()
        .map(|doc| {
            (
                doc.file().to_owned(),
                folo_store::encode(&node.store.get(*doc)),
            )
        })
        .collect();
    let contents = Contents {
        documents,
        packs: node.library.backup_packs()?,
        bases: read_bases(&node.store.dir().join(APPLIED))?,
    };
    let bytes = backup::write(contents, env!("CARGO_PKG_VERSION"), created)?;
    Ok((file_name(created), bytes))
}

/// The files of `dir` named as bases; a larger file isn't one.
fn read_bases(dir: &Path) -> Result<Entries, LibraryError> {
    let mut bases = Entries::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(bases),
        Err(error) => return Err(error.into()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let small = entry
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() <= limits::JSON_FILE);
        if backup::is_base_name(&name) && small {
            bases.insert(name, fs::read(entry.path())?);
        }
    }
    Ok(bases)
}

/// `folo-backup-<UTC date and time>.zip` (ADR-0038 D25).
fn file_name(created: u64) -> String {
    let (days, seconds) = (created / 86_400, created % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "folo-backup-{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z.zip",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// The date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Checks the whole backup, then replaces the documents, the library and the
/// bases with it (ADR-0038 D27). Nothing changes when a check fails.
///
/// # Errors
///
/// The errors of `POST /system/restore`.
pub fn restore(node: &Node, bytes: &[u8]) -> Result<(), LibraryError> {
    let _alone = RESTORES.lock().unwrap_or_else(PoisonError::into_inner);
    let restored = backup::read(bytes)?;
    node.library.check_backup(&restored)?;
    let documents = documents_of(&restored)?;
    let data = node.store.dir().to_path_buf();
    // Held until the moves are done, so no apply writes a base meanwhile
    let _writes = apply_api::show_writes();
    let staging = data.join(RESTORE_DIR);
    let result = stage(&staging, &restored).and_then(|()| {
        node.store.restore(documents, |files| {
            stage_documents(&data, files)?;
            // From the marker on the restore is binding: a move that fails now is
            // finished at the next start, and the node goes on with the backup's
            // state in memory so a later write can't undo the moves already made
            node.library.replace_all(&restored.packs, || {
                if let Err(error) = move_into_place(&data) {
                    log::error!("restore: a move failed, the next start finishes it: {error}");
                }
                Ok(())
            })
        })
    });
    if let Err(error) = &result {
        if data.join(MARKER).exists() {
            // The moves are the node's to finish, at its next start
            log::error!("restore: stopped after its marker was written: {error}");
        } else {
            let _ = fs::remove_dir_all(&staging);
        }
    }
    result?;
    node.refresh_engine();
    log::info!("restore: the documents, the library and the bases are the backup's");
    Ok(())
}

/// The backup's documents, clamped; a newer schema is `incompatible`.
fn documents_of(restored: &Restored) -> Result<Vec<(Doc, Value)>, LibraryError> {
    BACKUP_DOCS
        .iter()
        .map(|doc| {
            let bytes = &restored.documents[doc.file()];
            let newer = serde_json::from_slice::<Value>(bytes)
                .ok()
                .and_then(|v| v.get("schema_version")?.as_i64())
                .filter(|v| *v > SCHEMA_VERSION);
            if let Some(found) = newer {
                return Err(LibraryError::IncompatibleBackup {
                    what: "schema_version",
                    found,
                    supported: SCHEMA_VERSION,
                });
            }
            folo_store::parse(*doc, bytes)
                .map(|value| (*doc, value))
                .map_err(|e| match e {
                    folo_store::StoreError::Structural { why, .. } => {
                        LibraryError::structural(doc.file(), why)
                    }
                    folo_store::StoreError::Io { error, .. } => error.into(),
                })
        })
        .collect()
}

/// Writes the packs and the bases into a clean `restore/`.
fn stage(staging: &Path, restored: &Restored) -> Result<(), LibraryError> {
    let _ = fs::remove_dir_all(staging);
    fs::create_dir_all(staging)?;
    backup::stage(&staging.join(LIBRARY), &staging.join(APPLIED), restored)
}

/// Writes the documents into `restore/`, then the marker that makes the
/// restore binding: from there, the node finishes it even after a crash.
fn stage_documents(data: &Path, documents: &[(Doc, Vec<u8>)]) -> Result<(), LibraryError> {
    for (doc, bytes) in documents {
        files::write_atomic(&data.join(RESTORE_DIR).join(doc.file()), bytes, false)?;
    }
    // Every name in restore/ is durable before the marker says it is complete
    sync_tree(&data.join(RESTORE_DIR));
    files::write_atomic(&data.join(MARKER), b"restore/ is complete\n", false)?;
    Ok(())
}

/// Makes the directory entries below `dir` durable where the platform allows it.
fn sync_tree(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                sync_tree(&entry.path());
            }
        }
    }
    files::sync_dir(dir);
}

/// Moves what `restore/` holds into the data folder and removes the marker.
/// It can run again after an interruption: what has moved is no longer there.
fn move_into_place(data: &Path) -> io::Result<()> {
    let staging = data.join(RESTORE_DIR);
    for name in DOCUMENTS {
        let from = staging.join(name);
        if from.is_file() {
            fs::rename(&from, data.join(name))?;
        }
    }
    for name in [LIBRARY, APPLIED] {
        let from = staging.join(name);
        if from.is_dir() {
            let to = data.join(name);
            match fs::remove_dir_all(&to) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            fs::rename(&from, &to)?;
        }
    }
    fs::remove_file(data.join(MARKER))?;
    match fs::remove_dir_all(&staging) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// At the start of a node: finishes a restore whose marker was left behind,
/// or removes a `restore/` that was never completed (ADR-0038 D27).
///
/// # Errors
///
/// When the moves can't be finished; the node doesn't start with the data
/// folder half restored.
pub fn recover(data: &Path) -> io::Result<()> {
    if data.join(MARKER).exists() {
        log::warn!("restore: finishing a restore that was interrupted");
        return move_into_place(data);
    }
    match fs::remove_dir_all(data.join(RESTORE_DIR)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            log::warn!("restore: an unfinished restore/ can't be removed: {error}");
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_use_the_utc_date_and_time() {
        assert_eq!(file_name(0), "folo-backup-19700101T000000Z.zip");
        assert_eq!(file_name(1_782_493_445), "folo-backup-20260626T170405Z.zip");
        assert_eq!(file_name(951_782_400), "folo-backup-20000229T000000Z.zip");
    }
}
