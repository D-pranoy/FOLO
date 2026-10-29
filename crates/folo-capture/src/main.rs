//! `folo-capture`, the display capture agent (docs/integrations/display-capture.md).

use std::process::ExitCode;

fn main() -> ExitCode {
    folo_capture::cli::main()
}
