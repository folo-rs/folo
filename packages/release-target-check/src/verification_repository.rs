use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cargo_release_plan::{RunInput, RunOutcome, run};
use ohno::AppError;

use crate::{Metadata, Repository, capture};

/// Supplies external evidence to the shared verification sequence.
///
/// The verifier owns ordering, command inputs, rechecks and verdict handling; this boundary
/// connects those decisions to the candidate checkout. See docs/implementation.md.
pub(crate) trait VerificationRepository {
    fn ensure_clean_head(&self) -> Result<(), AppError>;
    fn ensure_first_parent(&self, release_line: &str) -> Result<(), AppError>;
    fn require_tracked(&self, manifest: &Path) -> Result<PathBuf, AppError>;
    fn capture(&self, program: &str, arguments: &[&OsStr]) -> Result<Vec<u8>, AppError>;
    fn validate_inputs(&self, metadata: &Metadata, manifest: &Path) -> Result<(), AppError>;
    fn check(&self, input: &RunInput) -> Result<RunOutcome, AppError>;
}

impl VerificationRepository for Repository {
    // Real Git forwarding is covered by integration tests, not library mutation targets.
    #[cfg_attr(test, mutants::skip)]
    fn ensure_clean_head(&self) -> Result<(), AppError> {
        self.ensure_clean_head()
    }

    // Real Git forwarding is covered by integration tests.
    #[cfg_attr(test, mutants::skip)]
    fn ensure_first_parent(&self, release_line: &str) -> Result<(), AppError> {
        self.ensure_first_parent(release_line)
    }

    // Real filesystem and Git forwarding is covered by integration tests.
    #[cfg_attr(test, mutants::skip)]
    fn require_tracked(&self, manifest: &Path) -> Result<PathBuf, AppError> {
        self.require_tracked(manifest)
    }

    // The verifier constructs the command; this adapter only supplies its real execution context.
    #[cfg_attr(test, mutants::skip)]
    fn capture(&self, program: &str, arguments: &[&OsStr]) -> Result<Vec<u8>, AppError> {
        capture(program, arguments, &self.root)
    }

    // Metadata's input-selection decisions have in-process coverage; filesystem wiring does not.
    #[cfg_attr(test, mutants::skip)]
    fn validate_inputs(&self, metadata: &Metadata, manifest: &Path) -> Result<(), AppError> {
        metadata.validate_inputs(self, manifest)
    }

    // The real release checker reads Git and filesystem state and belongs to integration tests.
    #[cfg_attr(test, mutants::skip)]
    fn check(&self, input: &RunInput) -> Result<RunOutcome, AppError> {
        run(input)
    }
}
