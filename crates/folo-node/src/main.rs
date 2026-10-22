//! `folo`, the FOLO node (docs/architecture/control-plane.md#command-line).

use std::process::ExitCode;

fn main() -> ExitCode {
    folo_node::cli::main()
}
