use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crp_diag::Stderr;
use crp_publication::PublicationOutput;
use crp_publication::legacy::run_binaries;
use ohno::AppError;

/// Runs the bootstrap protocol while the old release workflow remains selected.
#[must_use]
pub fn run() -> ExitCode {
    match dispatch() {
        Ok(Some(plan)) => {
            println!("{plan}");
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch() -> Result<Option<String>, AppError> {
    let diagnostics = PublicationOutput::new(env!("CARGO_PKG_VERSION"), true, Arc::new(Stderr));
    let summary = env::var_os("GITHUB_STEP_SUMMARY").map(PathBuf::from);
    run_binaries(
        env::args_os().skip(1),
        &env::current_dir()?,
        summary.as_deref(),
        &diagnostics,
    )
}
