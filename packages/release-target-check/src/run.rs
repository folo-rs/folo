use std::env::args_os;
use std::process::ExitCode;

use crp_diag::{Stderr, Verbose};
use crp_publication::legacy::verify_candidate;

/// Adapts the existing verifier process to publication's typed candidate check.
#[must_use]
pub fn run() -> ExitCode {
    match verify_candidate(args_os().skip(1), Verbose::new(true, &Stderr)) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
