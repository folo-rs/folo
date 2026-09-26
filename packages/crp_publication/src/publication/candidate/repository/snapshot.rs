use std::fs;
use std::path::{Path, PathBuf};

use crp_workspace::snapshot::SourceSnapshot;
use ohno::AppError;

use crate::publication::candidate::repository::validation::{
    relative_input, validate_around, validate_commit, validate_history, validate_index,
    validate_status,
};

/// Binds all verification reads to the caller's immutable candidate checkout.
#[derive(Debug)]
pub struct Repository {
    source: SourceSnapshot,
    commit: String,
}

impl Repository {
    #[must_use]
    // Trivial accessor, exercised by the integration fixture.
    #[cfg_attr(test, mutants::skip)]
    pub fn root(&self) -> &Path {
        self.source.root()
    }

    // Canonicalization and Git discovery require the real-system integration boundary.
    #[cfg_attr(test, mutants::skip)]
    pub fn discover(manifest: &Path, commit: &str) -> Result<Self, AppError> {
        Ok(Self {
            source: SourceSnapshot::discover(manifest)?,
            commit: commit.to_owned(),
        })
    }

    // The integration suite covers these Git queries; validation decisions are unit-tested.
    #[cfg_attr(test, mutants::skip)]
    pub fn ensure_clean_head(&self) -> Result<(), AppError> {
        let head = self.source.head()?;
        validate_commit(&head, &self.commit)?;
        let status = self.source.status()?;
        validate_status(&status)?;
        // Status trusts assume-unchanged and skip-worktree flags. Those flags cannot certify
        // released source bytes, even when the index itself names the correct commit.
        let files = self.source.index()?;
        validate_index(&files)
    }

    // Trivial wiring of the real Git adapter into the unit-tested recheck sequence.
    #[cfg_attr(test, mutants::skip)]
    pub fn checked<T>(
        &self,
        operation: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        validate_around(|| self.ensure_clean_head(), operation)
    }

    // Git supplies commit resolution and history; pure comparisons remain mutation-tested.
    #[cfg_attr(test, mutants::skip)]
    pub fn ensure_first_parent(&self, release_line: &str) -> Result<(), AppError> {
        for commit in [&self.commit, release_line] {
            let resolved = self.source.resolve(commit)?;
            validate_commit(&resolved, commit)?;
        }
        let history = self.source.first_parent(release_line)?;
        validate_history(&history, &self.commit, release_line)
    }

    // Canonicalization and tracked-file queries are integration-tested real-system adapters.
    #[cfg_attr(test, mutants::skip)]
    pub fn require_tracked(&self, path: &Path) -> Result<PathBuf, AppError> {
        let path = canonicalize(path)?;
        let relative = relative_input(&path, self.source.root())?;
        self.source.tracked(relative)?;
        Ok(path)
    }
}

// The integration suite covers native filesystem lookup and its error propagation.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn canonicalize(path: &Path) -> Result<PathBuf, AppError> {
    fs::canonicalize(path).map_err(|error| {
        VerificationError::caused_by(
            format!("cannot locate release input {}", path.display()),
            error,
        )
        .into()
    })
}

/// Identifies evidence that cannot certify the requested release snapshot.
#[ohno::error]
#[display("{reason}")]
pub(crate) struct VerificationError {
    reason: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Repository: RefUnwindSafe, UnwindSafe);
}
