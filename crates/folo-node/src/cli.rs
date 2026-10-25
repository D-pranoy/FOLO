//! The `folo` command line (docs/architecture/control-plane.md#command-line):
//! the commands, their flags and exit statuses, and finding the home and
//! settings every command starts from.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{Args, CommandFactory as _, Parser, Subcommand};
use folo_settings::folders::{self, Folders, Location, Platform};
use folo_settings::{Launch, Layered};

/// Exit statuses.
pub const EXIT_OK: u8 = 0;
pub const EXIT_FAILED: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
/// `run`: another node holds the home. `status`, `reload`: no node runs.
/// `config import`: the node runs.
pub const EXIT_NOT_HERE: u8 = 3;

#[derive(Parser, Debug)]
#[command(
    name = "folo",
    version,
    about = "The FOLO node. Without a command, it runs the node."
)]
pub struct Cli {
    #[command(flatten)]
    pub location: LocationArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub run: RunArgs,
}

impl Cli {
    /// Parses the command line. `--home` and `--service` go anywhere; the
    /// node's own flags only without a command or after `run`. (clap's
    /// `args_conflicts_with_subcommands` would refuse `folo --home x status`.)
    ///
    /// # Errors
    ///
    /// A usage error, as clap reports it (exit status 2).
    pub fn try_parse_args<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let cli = Self::try_parse_from(args)?;
        if cli.command.is_some() && cli.run.is_given() {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "the node's flags go after `folo run`, or without a command",
            ));
        }
        Ok(cli)
    }
}

/// Where the node's home is; every command takes these.
#[derive(Args, Debug, Clone, Default)]
pub struct LocationArgs {
    /// Use this folder as the home: node.toml, the data and the logs.
    #[arg(long, global = true, value_name = "FOLDER")]
    pub home: Option<PathBuf>,
    /// Use the service's folders.
    #[arg(long, global = true)]
    pub service: bool,
}

/// `folo run`'s flags: the deployment overrides, and `--managed`.
#[derive(Args, Debug, Clone, Default)]
pub struct RunArgs {
    /// Started by the desktop app: node.toml only, no other flags.
    #[arg(long)]
    pub managed: bool,
    /// controller or source.
    #[arg(long, value_name = "ROLE")]
    pub role: Option<String>,
    /// The address to listen on while remote access is on.
    #[arg(long, value_name = "ADDRESS")]
    pub address: Option<String>,
    /// The port to listen on.
    #[arg(long, value_name = "PORT")]
    pub port: Option<String>,
    /// The data folder.
    #[arg(long = "data-dir", value_name = "FOLDER")]
    pub data_dir: Option<String>,
    /// Remote access: on or off.
    #[arg(long = "remote-access", value_name = "ON|OFF")]
    pub remote_access: Option<String>,
    /// A host name or address for the Host check; may be repeated.
    #[arg(long = "allowed-host", value_name = "HOST")]
    pub allowed_host: Vec<String>,
    /// error, warn, info, debug or trace.
    #[arg(long = "log-level", value_name = "LEVEL")]
    pub log_level: Option<String>,
    /// The local source: on or off.
    #[arg(long = "local-source", value_name = "ON|OFF")]
    pub local_source: Option<String>,
}

impl RunArgs {
    /// Whether any of these flags was given.
    #[must_use]
    pub fn is_given(&self) -> bool {
        self.managed || !self.flags().is_empty()
    }

    /// The overrides as setting paths and values.
    #[must_use]
    pub fn flags(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let mut one = |path: &'static str, value: &Option<String>| {
            if let Some(value) = value {
                out.push((path, value.clone()));
            }
        };
        one("role", &self.role);
        one("listen.address", &self.address);
        one("listen.port", &self.port);
        one("data_dir", &self.data_dir);
        one("remote_access.enabled", &self.remote_access);
        one("log_level", &self.log_level);
        one("local_source.enabled", &self.local_source);
        for host in &self.allowed_host {
            out.push(("remote_access.allowed_hosts", host.clone()));
        }
        out
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the node until it is stopped.
    Run(RunArgs),
    /// The node's status, from the running node or from the home's files.
    Status {
        /// Print the status as JSON.
        #[arg(long)]
        json: bool,
        /// The port the node listens on, if not node.toml's.
        #[arg(long, value_name = "PORT")]
        port: Option<String>,
    },
    /// Apply node.toml and the documents to the running node.
    Reload {
        /// The port the node listens on, if not node.toml's.
        #[arg(long, value_name = "PORT")]
        port: Option<String>,
    },
    /// The configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// The master admin key (arrives with RB-15).
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// Pairing recovery (arrives with RB-16).
    Pair {
        #[command(subcommand)]
        command: PairCommand,
    },
    /// Join a controller with an invite (arrives with RB-16).
    Join {
        /// The invite link.
        code: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Import a legacy studio.json, from the given file or else as a first run would.
    Import {
        /// The studio.json to import.
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TokenCommand {
    Show,
    Rotate,
}

#[derive(Subcommand, Debug)]
pub enum PairCommand {
    Reset,
}

/// A command that couldn't go on: its exit status and what to say.
#[derive(Debug)]
pub struct Failure {
    pub status: u8,
    pub message: String,
}

impl Failure {
    pub fn new(status: u8, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Everything a command needs to know about its node: folders and settings.
#[derive(Debug, Clone)]
pub struct Place {
    pub folders: Folders,
    pub layered: Layered,
    pub launch: Launch,
    /// Launched with `--service`, whatever folders it uses.
    pub service: bool,
    pub exe_dir: PathBuf,
    pub cwd: PathBuf,
}

/// The environment, as the settings and folders read it.
#[must_use]
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Finds the home and reads the settings: `node.toml`, then for a
/// foreground launch the variables and `flags`.
///
/// # Errors
///
/// When the home can't be found or `node.toml` can't be read.
pub fn locate(
    location: &LocationArgs,
    flags: &[(&'static str, String)],
    launch: Launch,
) -> Result<Place, Failure> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let home_override = location.home.clone().or_else(|| match launch {
        Launch::Foreground => env_var("FOLO_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        Launch::Managed => None,
    });
    let place = Location {
        home: home_override,
        service: location.service,
        portable: exe_dir.join("portable.ini").is_file(),
        exe_dir: exe_dir.clone(),
        cwd: cwd.clone(),
        env: &env_var,
        platform: Platform::current(),
    };
    let (mode, home) =
        folders::home(&place).map_err(|e| Failure::new(EXIT_FAILED, e.to_string()))?;
    let settings_file = home.join("node.toml");
    let file = match std::fs::read_to_string(&settings_file) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(Failure::new(
                EXIT_FAILED,
                format!("{}: {error}", settings_file.display()),
            ));
        }
    };
    let layered = folo_settings::layer(file.as_deref(), &env_var, flags, launch)
        .map_err(|e| Failure::new(EXIT_FAILED, format!("{}: {e}", settings_file.display())))?;
    let folders = folders::folders(&place, mode, &home, &layered.settings.data_dir)
        .map_err(|e| Failure::new(EXIT_FAILED, e.to_string()))?;
    Ok(Place {
        folders,
        layered,
        launch,
        service: location.service,
        exe_dir,
        cwd,
    })
}

/// Runs the command line, as `main` does.
#[must_use]
pub fn main() -> ExitCode {
    let cli = Cli::try_parse_args(std::env::args_os()).unwrap_or_else(|error| error.exit());
    let status = match cli.command {
        None => crate::node::main(&cli.location, &cli.run),
        Some(Command::Run(run)) => crate::node::main(&cli.location, &run),
        Some(Command::Status { json, port }) => crate::status::command(&cli.location, port, json),
        Some(Command::Reload { port }) => crate::status::reload_command(&cli.location, port),
        Some(Command::Config {
            command: ConfigCommand::Import { path },
        }) => crate::import::command(&cli.location, path),
        Some(Command::Token { .. }) => not_yet("folo token", "RB-15"),
        Some(Command::Pair { .. }) => not_yet("folo pair reset", "RB-16"),
        Some(Command::Join { .. }) => not_yet("folo join", "RB-16"),
    };
    ExitCode::from(status)
}

/// A command whose run book hasn't landed yet.
fn not_yet(command: &str, run_book: &str) -> u8 {
    eprintln!("{command} isn't available yet: it arrives with {run_book}.");
    EXIT_FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_command_runs_the_node_with_its_flags() {
        let cli = Cli::try_parse_args(["folo", "--role", "source", "--port", "9000"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(
            cli.run.flags(),
            [
                ("role", "source".to_owned()),
                ("listen.port", "9000".to_owned())
            ]
        );
    }

    #[test]
    fn location_flags_work_with_every_command() {
        let cli = Cli::try_parse_args(["folo", "status", "--json", "--home", "x"]).unwrap();
        assert_eq!(cli.location.home, Some(PathBuf::from("x")));
        assert!(matches!(
            cli.command,
            Some(Command::Status { json: true, .. })
        ));
        let cli = Cli::try_parse_args(["folo", "--service", "reload"]).unwrap();
        assert!(cli.location.service);
        let cli = Cli::try_parse_args(["folo", "--home", "x", "config", "import"]).unwrap();
        assert_eq!(cli.location.home, Some(PathBuf::from("x")));
        let cli = Cli::try_parse_args(["folo", "--home", "x", "run", "--port", "9000"]).unwrap();
        assert_eq!(cli.location.home, Some(PathBuf::from("x")));
    }

    #[test]
    fn the_nodes_flags_need_run_or_no_command() {
        for args in [
            &["folo", "--port", "9000", "status"][..],
            &["folo", "--managed", "run"][..],
            &["folo", "--log-level", "debug", "reload"][..],
        ] {
            let error = Cli::try_parse_args(args).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::ArgumentConflict, "{args:?}");
            assert_eq!(error.exit_code(), i32::from(EXIT_USAGE), "{args:?}");
        }
        let cli = Cli::try_parse_args(["folo", "status", "--port", "9000"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Status { port: Some(_), .. })
        ));
    }

    #[test]
    fn list_flags_repeat() {
        let cli =
            Cli::try_parse_args(["folo", "run", "--allowed-host", "a", "--allowed-host", "b"])
                .unwrap();
        let Some(Command::Run(run)) = cli.command else {
            panic!("not run");
        };
        assert_eq!(
            run.flags(),
            [
                ("remote_access.allowed_hosts", "a".to_owned()),
                ("remote_access.allowed_hosts", "b".to_owned())
            ]
        );
    }

    #[test]
    fn every_flag_is_in_the_control_plane() {
        let text = include_str!("../../../docs/architecture/control-plane.md");
        for spec in &folo_settings::SETTINGS {
            assert!(
                text.contains(&format!("`{}`", spec.variable)),
                "{}",
                spec.variable
            );
            assert!(text.contains(&format!("`{}", spec.flag)), "{}", spec.flag);
        }
        for command in [
            "folo run",
            "folo status",
            "folo reload",
            "folo config import",
        ] {
            assert!(text.contains(&format!("`{command}")), "{command}");
        }
    }

    #[test]
    fn usage_errors_exit_with_2() {
        let error = Cli::try_parse_args(["folo", "--bogus"]).unwrap_err();
        assert_eq!(error.exit_code(), i32::from(EXIT_USAGE));
    }
}
