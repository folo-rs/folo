use std::env::args_os;
use std::process::ExitCode;

use crate::cli::Cli;
use crate::verify::verify;

/// Verifies the candidate selected by this process's command-line arguments.
///
/// Prints the result to stdout on success or the error to stderr on failure,
/// returning the corresponding process exit code.
#[must_use]
pub fn run() -> ExitCode {
    match Cli::parse(args_os().skip(1)).and_then(|cli| verify(&cli)) {
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
