//! The `folo-capture` command line and its exit statuses (ADR-0037 D2).

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use folo_capture_core::types::ScreensListResult;

use crate::backends::BackendChoice;
use crate::environment::Environment;

/// Exit statuses: 0 after a stop request or a restart the node asked for; 1
/// when the link was refused or the node lost (or the backend failed); 2 when
/// the environment lacks the link's URL or token, or the command line is wrong.
pub const EXIT_OK: u8 = 0;
pub const EXIT_FAILED: u8 = 1;
pub const EXIT_USAGE: u8 = 2;

#[derive(Parser, Debug)]
#[command(
    name = "folo-capture",
    version,
    about = "The FOLO display capture agent. Without a command, it runs."
)]
pub struct Cli {
    /// The capture backend: auto, dda, wgc, gdi, portal, synthetic or replay:<file>.
    #[arg(long, global = true, value_name = "BACKEND", default_value = "auto")]
    pub backend: BackendChoice,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum Command {
    /// Dial the node's capture link and capture on its request. It needs
    /// `FOLO_CAPTURE_LINK` and `FOLO_CAPTURE_TOKEN` in its environment.
    Run,
    /// List the screens without capturing anything or asking for consent.
    Screens {
        /// Print `{"screens": [...]}`, as `screens.list` answers it.
        #[arg(long)]
        json: bool,
    },
}

/// Runs the command line, as `main` does.
#[must_use]
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let status = match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(&cli.backend),
        Command::Screens { json } => screens(&cli.backend, json),
    };
    ExitCode::from(status)
}

fn run(backend: &BackendChoice) -> u8 {
    let _environment = match Environment::read(|name| std::env::var(name).ok()) {
        Ok(environment) => environment,
        Err(missing) => {
            eprintln!("folo-capture: {missing}.");
            return EXIT_USAGE;
        }
    };
    if let Err(error) = backend.open() {
        eprintln!("folo-capture: {error}.");
        return EXIT_FAILED;
    }
    // The link client (the handshake, `capture.start`, frames and the
    // reconnecting) is the node-side change's counterpart and comes with it.
    eprintln!("folo-capture: the capture link isn't built yet, so there is nothing to run for.");
    EXIT_FAILED
}

fn screens(backend: &BackendChoice, json: bool) -> u8 {
    let listed = backend
        .open()
        .and_then(|mut backend| backend.list_screens());
    let screens = match listed {
        Ok(screens) => screens,
        Err(error) => {
            eprintln!("folo-capture: {error}.");
            return EXIT_FAILED;
        }
    };
    if json {
        match serde_json::to_string(&ScreensListResult { screens }) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("folo-capture: {error}.");
                return EXIT_FAILED;
            }
        }
    } else {
        for screen in screens {
            let primary = if screen.primary { " (primary)" } else { "" };
            println!(
                "{}\t{}\t{}x{}{primary}",
                screen.id, screen.name, screen.width, screen.height
            );
        }
    }
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_backend_comes_before_or_after_the_command() {
        let before =
            Cli::try_parse_from(["folo-capture", "--backend", "synthetic", "run"]).unwrap();
        let after = Cli::try_parse_from([
            "folo-capture",
            "screens",
            "--json",
            "--backend",
            "synthetic",
        ])
        .unwrap();
        assert_eq!(before.backend, BackendChoice::Synthetic);
        assert_eq!(after.backend, BackendChoice::Synthetic);
        assert_eq!(
            Cli::try_parse_from(["folo-capture"]).unwrap().backend,
            BackendChoice::Auto
        );
    }
}
