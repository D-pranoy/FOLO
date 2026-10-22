//! One node per home (ADR-0034 D4): an operating-system lock on `folo.lock`,
//! which holds the node's process id. On Unix it's a file lock; on Windows,
//! where a file lock would also stop others from reading the id, the file
//! is kept open for writing with others allowed to read only. Either ends
//! with the process.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// `FILE_SHARE_READ`.
#[cfg(windows)]
const SHARE_READ: u32 = 0x1;
/// `FILE_SHARE_READ | FILE_SHARE_WRITE`, for reading a held lock's id.
#[cfg(windows)]
const SHARE_ALL: u32 = 0x3;
/// `ERROR_SHARING_VIOLATION`.
#[cfg(windows)]
const SHARING_VIOLATION: i32 = 32;

/// A held lock on a home.
#[derive(Debug)]
pub struct HomeLock {
    _file: File,
    path: PathBuf,
}

impl HomeLock {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Why the lock can't be taken.
#[derive(Debug)]
pub enum LockError {
    /// Another node holds it; its process id when the file says.
    Held(Option<u32>),
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held(Some(pid)) => write!(f, "another node (process {pid}) runs on this home"),
            Self::Held(None) => f.write_str("another node runs on this home"),
            Self::Io(error) => write!(f, "the home's lock can't be taken: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Takes the lock on `path` and writes this process's id into it.
///
/// # Errors
///
/// [`LockError::Held`] when another process holds it, or [`LockError::Io`].
pub fn acquire(path: &Path) -> Result<HomeLock, LockError> {
    let mut file = open_exclusive(path)?;
    file.set_len(0).map_err(LockError::Io)?;
    file.seek(SeekFrom::Start(0)).map_err(LockError::Io)?;
    writeln!(file, "{}", std::process::id()).map_err(LockError::Io)?;
    file.flush().map_err(LockError::Io)?;
    Ok(HomeLock {
        _file: file,
        path: path.to_path_buf(),
    })
}

/// Whether a node holds the lock on `path`, with its process id when known.
/// `None` when it's free.
#[must_use]
pub fn holder(path: &Path) -> Option<Option<u32>> {
    if !path.exists() {
        return None;
    }
    match open_exclusive(path) {
        Ok(_) | Err(LockError::Io(_)) => None,
        Err(LockError::Held(pid)) => Some(pid),
    }
}

#[cfg(unix)]
fn open_exclusive(path: &Path) -> Result<File, LockError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(LockError::Held(read_pid(path))),
        Err(std::fs::TryLockError::Error(error)) => Err(LockError::Io(error)),
    }
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> Result<File, LockError> {
    use std::os::windows::fs::OpenOptionsExt as _;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(SHARE_READ)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(SHARING_VIOLATION) {
                LockError::Held(read_pid(path))
            } else {
                LockError::Io(error)
            }
        })
}

fn read_pid(path: &Path) -> Option<u32> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(SHARE_ALL);
    }
    let mut text = String::new();
    options.open(path).ok()?.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let dir = std::env::temp_dir().join(format!("folo-lock-{}", folo_store::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("folo.lock");
        assert_eq!(holder(&path), None);
        let lock = acquire(&path).unwrap();
        assert_eq!(holder(&path), Some(Some(std::process::id())));
        assert!(matches!(acquire(&path), Err(LockError::Held(Some(_)))));
        drop(lock);
        assert_eq!(holder(&path), None);
        let again = acquire(&path).unwrap();
        assert_eq!(again.path(), path);
        drop(again);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
