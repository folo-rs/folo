//! Binary entry point for release-target-check.

use std::process::ExitCode;

use release_target_check::run;

fn main() -> ExitCode {
    run()
}
