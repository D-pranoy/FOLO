//! The node's home, data and log folders (RB-06 step 3,
//! docs/architecture/control-plane.md#folders, ADR-0034 D1). Pure: the
//! caller passes the environment, the executable's folder and whether
//! `portable.ini` is there, so both platforms' rules can be tested anywhere.

use std::path::{Path, PathBuf};

/// The launch that chose the folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `--home` or `FOLO_HOME`; the Docker image sets the latter.
    Custom,
    /// `portable.ini` beside the executable.
    Portable,
    /// `--service`.
    Service,
    /// Everything else.
    User,
}

impl Mode {
    /// The name `folo status` reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Portable => "portable",
            Self::Service => "service",
            Self::User => "user",
        }
    }
}

/// Whose folder conventions apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    /// Linux and other Unix systems: the XDG Base Directory conventions.
    Unix,
}

impl Platform {
    /// The platform this build runs on.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// What the folders are found from.
pub struct Location<'a> {
    /// `--home`, or `FOLO_HOME` for a foreground launch.
    pub home: Option<PathBuf>,
    /// `--service`.
    pub service: bool,
    /// Whether `portable.ini` is beside the executable.
    pub portable: bool,
    /// The executable's folder.
    pub exe_dir: PathBuf,
    /// The working directory, for relative paths.
    pub cwd: PathBuf,
    /// The environment.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub platform: Platform,
}

/// Why the folders can't be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldersError {
    /// The variable the platform's folders start from isn't set.
    Unset(&'static str),
}

impl std::fmt::Display for FoldersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset(name) => write!(f, "{name} isn't set, so the node has no home folder"),
        }
    }
}

impl std::error::Error for FoldersError {}

/// The node's folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folders {
    pub mode: Mode,
    /// Holds `node.toml` and `folo.lock`.
    pub home: PathBuf,
    /// Holds the documents, `secrets.json` and `library/`.
    pub data: PathBuf,
    /// Holds `folo.log` and its older files.
    pub logs: PathBuf,
}

impl Folders {
    /// The settings file.
    #[must_use]
    pub fn settings_file(&self) -> PathBuf {
        self.home.join("node.toml")
    }

    /// The lock that keeps one node per home.
    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.home.join("folo.lock")
    }

    /// The log file.
    #[must_use]
    pub fn log_file(&self) -> PathBuf {
        self.logs.join("folo.log")
    }
}

/// The home and the launch that chose it: the first of `--home` or
/// `FOLO_HOME`, `portable.ini`, `--service` and the user's folders.
///
/// # Errors
///
/// [`FoldersError::Unset`] when a variable the user's folders need isn't set.
pub fn home(location: &Location<'_>) -> Result<(Mode, PathBuf), FoldersError> {
    if let Some(home) = &location.home {
        return Ok((Mode::Custom, absolute(&location.cwd, home)));
    }
    if location.portable {
        return Ok((Mode::Portable, location.exe_dir.join("data")));
    }
    if location.service {
        let home = match location.platform {
            Platform::Windows => {
                let base =
                    (location.env)("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
                PathBuf::from(base).join("FOLO")
            }
            Platform::Unix => PathBuf::from("/var/lib/folo"),
        };
        return Ok((Mode::Service, home));
    }
    let home = match location.platform {
        Platform::Windows => windows_user(location)?,
        Platform::Unix => xdg(location, "XDG_CONFIG_HOME", ".config")?.join("folo"),
    };
    Ok((Mode::User, home))
}

/// All three folders, given the home and the `data_dir` setting (empty for
/// the default; relative to the home otherwise).
///
/// # Errors
///
/// [`FoldersError::Unset`] when a variable the user's folders need isn't set.
pub fn folders(
    location: &Location<'_>,
    mode: Mode,
    home: &Path,
    data_dir: &str,
) -> Result<Folders, FoldersError> {
    let xdg_user = mode == Mode::User && location.platform == Platform::Unix;
    let data = if !data_dir.is_empty() {
        absolute(home, Path::new(data_dir))
    } else if xdg_user {
        xdg(location, "XDG_DATA_HOME", ".local/share")?.join("folo")
    } else {
        home.to_path_buf()
    };
    let logs = if xdg_user && data_dir.is_empty() {
        xdg(location, "XDG_STATE_HOME", ".local/state")?
            .join("folo")
            .join("logs")
    } else {
        data.join("logs")
    };
    Ok(Folders {
        mode,
        home: home.to_path_buf(),
        data,
        logs,
    })
}

fn windows_user(location: &Location<'_>) -> Result<PathBuf, FoldersError> {
    (location.env)("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|base| PathBuf::from(base).join("FOLO"))
        .ok_or(FoldersError::Unset("LOCALAPPDATA"))
}

/// An XDG base directory: the variable when it holds an absolute (POSIX)
/// path, otherwise its default under `$HOME`.
fn xdg(location: &Location<'_>, variable: &str, fallback: &str) -> Result<PathBuf, FoldersError> {
    if let Some(value) = (location.env)(variable)
        && value.starts_with('/')
    {
        return Ok(PathBuf::from(value));
    }
    (location.env)("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(fallback))
        .ok_or(FoldersError::Unset("HOME"))
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Env(HashMap<&'static str, &'static str>);

    impl Env {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|v| (*v).to_owned())
        }
    }

    fn found(
        env: &[(&'static str, &'static str)],
        platform: Platform,
        home: Option<&str>,
        service: bool,
        portable: bool,
        data_dir: &str,
    ) -> Folders {
        let env = Env(env.iter().copied().collect());
        let lookup = |name: &str| env.get(name);
        let location = Location {
            home: home.map(PathBuf::from),
            service,
            portable,
            exe_dir: PathBuf::from(if platform == Platform::Windows {
                r"C:\FOLO"
            } else {
                "/opt/folo"
            }),
            cwd: PathBuf::from(if platform == Platform::Windows {
                r"C:\work"
            } else {
                "/work"
            }),
            env: &lookup,
            platform,
        };
        let (mode, home) = super::home(&location).unwrap();
        folders(&location, mode, &home, data_dir).unwrap()
    }

    // Paths are compared as strings joined the way this platform joins them,
    // so the tests hold on Windows and on Linux.
    fn p(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn user_mode_on_windows_keeps_everything_together() {
        let f = found(
            &[("LOCALAPPDATA", r"C:\Users\u\AppData\Local")],
            Platform::Windows,
            None,
            false,
            false,
            "",
        );
        assert_eq!(f.mode, Mode::User);
        assert_eq!(f.home, p(&[r"C:\Users\u\AppData\Local", "FOLO"]));
        assert_eq!(f.data, f.home);
        assert_eq!(f.logs, p(&[r"C:\Users\u\AppData\Local", "FOLO", "logs"]));
        assert_eq!(f.settings_file(), f.home.join("node.toml"));
        assert_eq!(f.lock_file(), f.home.join("folo.lock"));
    }

    #[test]
    fn user_mode_on_linux_follows_xdg() {
        let f = found(
            &[("HOME", "/home/u")],
            Platform::Unix,
            None,
            false,
            false,
            "",
        );
        assert_eq!(f.home, p(&["/home/u", ".config", "folo"]));
        assert_eq!(f.data, p(&["/home/u", ".local/share", "folo"]));
        assert_eq!(f.logs, p(&["/home/u", ".local/state", "folo", "logs"]));

        let env = [
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_DATA_HOME", "/dat"),
            ("XDG_STATE_HOME", "relative/is/ignored"),
        ];
        let f = found(&env, Platform::Unix, None, false, false, "");
        assert_eq!(f.home, p(&["/cfg", "folo"]));
        assert_eq!(f.data, p(&["/dat", "folo"]));
        assert_eq!(f.logs, p(&["/home/u", ".local/state", "folo", "logs"]));
    }

    #[test]
    fn data_dir_moves_the_data_and_the_logs() {
        let f = found(
            &[("HOME", "/home/u")],
            Platform::Unix,
            None,
            false,
            false,
            "/srv/folo",
        );
        assert_eq!(f.data, p(&["/srv/folo"]));
        assert_eq!(f.logs, p(&["/srv/folo", "logs"]));

        let f = found(&[], Platform::Unix, Some("/h"), false, false, "data");
        assert_eq!(f.data, p(&["/h", "data"]));
    }

    #[test]
    fn home_comes_first_then_portable_then_service() {
        let env = [("LOCALAPPDATA", r"C:\L"), ("ProgramData", r"C:\PD")];
        let f = found(&env, Platform::Windows, Some("exp"), true, true, "");
        assert_eq!(f.mode, Mode::Custom);
        assert_eq!(f.home, p(&[r"C:\work", "exp"]));
        assert_eq!(f.logs, p(&[r"C:\work", "exp", "logs"]));

        let f = found(&env, Platform::Windows, None, true, true, "");
        assert_eq!(f.mode, Mode::Portable);
        assert_eq!(f.home, p(&[r"C:\FOLO", "data"]));

        let f = found(&env, Platform::Windows, None, true, false, "");
        assert_eq!(f.mode, Mode::Service);
        assert_eq!(f.home, p(&[r"C:\PD", "FOLO"]));
        assert_eq!(f.data, f.home);

        let f = found(&[], Platform::Unix, None, true, false, "");
        assert_eq!(f.home, p(&["/var/lib/folo"]));
        assert_eq!(f.logs, p(&["/var/lib/folo", "logs"]));
    }

    #[test]
    fn missing_variables_are_reported() {
        let env = Env(HashMap::new());
        let lookup = |name: &str| env.get(name);
        for (platform, name) in [
            (Platform::Windows, "LOCALAPPDATA"),
            (Platform::Unix, "HOME"),
        ] {
            let location = Location {
                home: None,
                service: false,
                portable: false,
                exe_dir: PathBuf::new(),
                cwd: PathBuf::new(),
                env: &lookup,
                platform,
            };
            assert_eq!(super::home(&location), Err(FoldersError::Unset(name)));
        }
    }
}
