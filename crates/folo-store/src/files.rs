//! Atomic file writes (a temporary file beside the target, fsync, replace),
//! optionally readable by the file's owner only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Writes `data` to `path` atomically. A `private` file can only be read by
/// its owner: POSIX mode 0600, or on Windows an access list granting the
/// owner alone, without inherited entries.
///
/// # Errors
///
/// Any I/O error, or the access list failing to apply.
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let temporary = temporary_beside(path)?;
    let result = (|| {
        let mut file = create(&temporary, private)?;
        if private {
            restrict(&temporary)?;
        }
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        sync_dir(dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Makes an existing file readable by its owner only (see [`write_atomic`]).
///
/// # Errors
///
/// Any I/O error, or the access list failing to apply.
pub fn restrict(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        // OWNER RIGHTS (S-1-3-4) as the only entry: whoever owns the file
        // has full control, and nobody else has any access.
        let output = std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", "*S-1-3-4:F", "/Q"])
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "icacls failed: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            )))
        }
    }
}

fn temporary_beside(path: &Path) -> io::Result<PathBuf> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Ok(path.with_file_name(format!(".{name}.{}.tmp", hex(&bytes))))
}

/// Bytes as lowercase hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn create(path: &Path, private: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    options.open(path)
}

/// Makes the rename durable where the platform allows it.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
