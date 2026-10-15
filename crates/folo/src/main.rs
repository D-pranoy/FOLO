//! The FOLO node. Until RB-06 builds it, this binary only proves that the
//! workspace builds and runs on every target (RB-01 step 10).

use std::process::ExitCode;

const USAGE: &str = "Usage: folo [--help | --version]";

/// What to print for the first argument: `Ok` on stdout, `Err` on stderr.
fn reply(arg: Option<&str>) -> Result<String, String> {
    match arg {
        None | Some("--help" | "-h") => Ok(format!("FOLO node (not implemented yet)\n\n{USAGE}")),
        Some("--version" | "-V") => Ok(format!("folo {}", env!("CARGO_PKG_VERSION"))),
        Some(other) => Err(format!("Unknown argument: {other}\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match reply(arg.as_deref()) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(text) => {
            eprintln!("{text}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::reply;

    #[test]
    fn prints_the_version() {
        assert_eq!(
            reply(Some("--version")),
            Ok(format!("folo {}", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn help_is_the_default() {
        assert_eq!(reply(None), reply(Some("--help")));
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(reply(Some("--bogus")).is_err());
    }
}
