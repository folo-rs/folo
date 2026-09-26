use std::ffi::OsString;

use crp_diag::Verbose;
use ohno::AppError;

use crate::legacy::candidate_cli::Cli;
use crate::publication::candidate::{CandidateRequest, verify};

/// Adapts the private verifier CLI to the ordinary typed candidate operation.
pub fn verify_candidate(
    arguments: impl IntoIterator<Item = OsString>,
    notes: Verbose<'_>,
) -> Result<String, AppError> {
    let cli = Cli::parse(arguments)?;
    let notes = Verbose::new(cli.verbose, notes.sink());
    verify(
        &CandidateRequest {
            manifest_path: cli.manifest_path,
            commit: cli.commit,
            release_line: cli.release_line,
            packages: cli.packages,
            verbose: cli.verbose,
        },
        notes,
    )
}
