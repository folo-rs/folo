//! Entry point for the repository's inexpensive action-pairing relevance check.

use std::process::ExitCode;

fn main() -> ExitCode {
    benchmark_action_check::run()
}
