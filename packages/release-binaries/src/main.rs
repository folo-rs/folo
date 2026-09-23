//! Release workflow entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    release_binaries::run()
}
