//! Atomic file writes (a temporary file beside the target, fsync, replace),
//! optionally readable by the file's owner only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Writes `data` to `path` atomically. A `private` file can only be read by
/// its owner: POSIX mode 0600, or on Windows a protected access list whose
/// one entry gives the file's owner full control.
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
        owner_only(path)
    }
}

/// Replaces the file's whole access list with OWNER RIGHTS (S-1-3-4) as
/// its one entry, protected from inheritance: whoever owns the file has
/// full control, and nobody else has any access. (`icacls /inheritance:r`
/// would leave explicit entries, which a file gets from its creator's
/// defaults where its folder passes nothing on.)
#[cfg(windows)]
#[allow(unsafe_code)] // three Win32 calls; see the SAFETY notes
fn owner_only(path: &Path) -> io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SetFileSecurityW,
    };

    /// A protected list (`P`) allowing (`A`) all file access (`FA`) to the owner (`OW`)
    const OWNER_ONLY: &str = "D:P(A;;FA;;;OW)";
    let wide = |text: &OsStr| text.encode_wide().chain([0]).collect::<Vec<u16>>();
    let sddl = wide(OsStr::new(OWNER_ONLY));
    let file = wide(path.as_os_str());
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` ends with a NUL; on success the call allocates the
    // descriptor, which is freed below.
    let made = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if made == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `file` ends with a NUL, and `descriptor` is valid until freed.
    let set = unsafe {
        SetFileSecurityW(
            file.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = io::Error::last_os_error();
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW
    // above, and freed once.
    unsafe { LocalFree(descriptor) };
    if set == 0 { Err(error) } else { Ok(()) }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The access list's entries, as `icacls` lists them.
    #[cfg(windows)]
    fn entries(path: &Path) -> Vec<String> {
        let output = std::process::Command::new("icacls")
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(":("))
            .map(str::to_owned)
            .collect()
    }

    /// Explicit entries go as well as inherited ones: a file created where
    /// nothing is inherited gets its creator's default entries as explicit
    /// ones, as on CI's Windows runners.
    #[cfg(windows)]
    #[test]
    fn restricting_leaves_the_owner_alone() {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).unwrap();
        let dir = std::env::temp_dir().join(format!("folo-files-{}", hex(&bytes)));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret");
        fs::write(&path, b"one").unwrap();
        let granted = std::process::Command::new("icacls")
            .arg(&path)
            .args(["/grant", "*S-1-5-32-545:(R)", "/Q"]) // BUILTIN\Users, explicit
            .status()
            .unwrap();
        assert!(granted.success());
        assert!(entries(&path).len() > 1);

        restrict(&path).unwrap();
        let left = entries(&path);
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].ends_with("(F)"), "{left:?}");
        fs::write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn restricting_leaves_the_owner_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).unwrap();
        let dir = std::env::temp_dir().join(format!("folo-files-{}", hex(&bytes)));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret");
        fs::write(&path, b"one").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        restrict(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }
}
