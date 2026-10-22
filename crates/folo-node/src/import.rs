//! Importing a legacy `studio.json` (RB-06 step 9, ADR-0034 D10, ADR-0010
//! D5): on a first run, from the home or else the working directory, and on
//! demand with `folo config import`, through `folo-source migrate-v1`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use folo_settings::Launch;
use folo_supervisor::EngineCommand;

use crate::cli::{EXIT_FAILED, EXIT_NOT_HERE, EXIT_OK, LocationArgs, locate};
use crate::{lock, logs};

const LEGACY: &str = "studio.json";

/// The first legacy `studio.json`: in the home, then in the working directory.
#[must_use]
pub fn find(home: &Path, cwd: &Path) -> Option<PathBuf> {
    [home, cwd]
        .into_iter()
        .map(|dir| dir.join(LEGACY))
        .find(|path| path.is_file())
}

/// Runs the migration into `data`; its log lines go into the node's log.
///
/// # Errors
///
/// When the migration can't run or fails.
pub fn migrate(engine: &EngineCommand, studio: &Path, data: &Path) -> Result<(), String> {
    let mut command = Command::new(&engine.program);
    command
        .args(&engine.args)
        .arg("migrate-v1")
        .arg(studio)
        .arg(data)
        .stdin(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if folo_supervisor::is_folo_variable(&name) {
            command.env_remove(name);
        }
    }
    let output = command
        .output()
        .map_err(|e| format!("{} can't run: {e}", engine.program.display()))?;
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        logs::engine_line(line);
    }
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("folo-source migrate-v1 failed ({})", output.status))
    }
}

/// A first run: imports the legacy file when there is one. A failure is
/// logged and the node starts on its defaults; `folo config import` can
/// try again.
pub fn first_run(engine: &EngineCommand, home: &Path, cwd: &Path, data: &Path) {
    let Some(studio) = find(home, cwd) else {
        log::info!("first run: no {LEGACY} to import");
        return;
    };
    log::info!("first run: importing {}", studio.display());
    match migrate(engine, &studio, data) {
        Ok(()) => log::info!("imported {}", studio.display()),
        Err(why) => log::error!("importing {} failed: {why}", studio.display()),
    }
}

/// `folo config import [<studio.json>]`: 0 when imported, 1 on a failure,
/// 3 while the node runs.
#[must_use]
pub fn command(location: &LocationArgs, path: Option<PathBuf>) -> u8 {
    let place = match locate(location, &[], Launch::Foreground) {
        Ok(place) => place,
        Err(failure) => {
            eprintln!("{}", failure.message);
            return failure.status;
        }
    };
    let folders = &place.folders;
    if let Err(error) = std::fs::create_dir_all(&folders.home) {
        eprintln!("{}: {error}", folders.home.display());
        return EXIT_FAILED;
    }
    let _lock = match lock::acquire(&folders.lock_file()) {
        Ok(lock) => lock,
        Err(lock::LockError::Held(_)) => {
            eprintln!("a node runs on {}; stop it first", folders.home.display());
            return EXIT_NOT_HERE;
        }
        Err(error) => {
            eprintln!("{error}");
            return EXIT_FAILED;
        }
    };
    let Some(studio) = path.or_else(|| find(&folders.home, &place.cwd)) else {
        eprintln!(
            "no {LEGACY} in {} or {}; give its path",
            folders.home.display(),
            place.cwd.display()
        );
        return EXIT_FAILED;
    };
    logs::init(log::LevelFilter::Info);
    let engine = EngineCommand::find(&place.exe_dir);
    match migrate(&engine, &studio, &folders.data) {
        Ok(()) => {
            println!(
                "imported {} into {}",
                studio.display(),
                folders.data.display()
            );
            EXIT_OK
        }
        Err(why) => {
            eprintln!("{why}");
            EXIT_FAILED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_comes_before_the_working_directory() {
        let root = std::env::temp_dir().join(format!("folo-import-{}", folo_store::new_id()));
        let (home, cwd) = (root.join("home"), root.join("cwd"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        assert_eq!(find(&home, &cwd), None);
        std::fs::write(cwd.join(LEGACY), b"{}").unwrap();
        assert_eq!(find(&home, &cwd), Some(cwd.join(LEGACY)));
        std::fs::write(home.join(LEGACY), b"{}").unwrap();
        assert_eq!(find(&home, &cwd), Some(home.join(LEGACY)));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
