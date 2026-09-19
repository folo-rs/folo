use std::io;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// Rejects repository spellings that cannot safely identify literal REST path segments.
#[ohno::error]
#[display("GitHub repository must have the form `owner/name`, got '{value}'")]
pub(crate) struct InvalidRepositoryError {
    value: String,
}

// Ohno's dynamic source core prevents automatic unwind-safety inference. These immutable
// leaves expose no mutation; this rationale covers every manual marker impl in this module
// and must be re-evaluated if mutation becomes observable.
impl UnwindSafe for InvalidRepositoryError {}
impl RefUnwindSafe for InvalidRepositoryError {}

/// Keeps internally supplied project namespaces usable in markers and job identities.
#[ohno::error]
#[display("Action instance must contain only ASCII letters, digits, `.`, `-` or `_`")]
pub(crate) struct InvalidInstanceError;

impl UnwindSafe for InvalidInstanceError {}
impl RefUnwindSafe for InvalidInstanceError {}

/// Prevents refs or abbreviated identities from entering frozen report ownership.
#[ohno::error]
#[display("Commit ID must be a full 40-digit hexadecimal SHA, got '{value}'")]
pub(crate) struct InvalidCommitShaError {
    value: String,
}

impl UnwindSafe for InvalidCommitShaError {}
impl RefUnwindSafe for InvalidCommitShaError {}

/// Identifies missing repository context before a command constructs its GitHub client.
#[ohno::error]
#[display("Neither `--repository` nor GITHUB_REPOSITORY identifies the repository")]
pub(crate) struct MissingRepositoryError;

impl UnwindSafe for MissingRepositoryError {}
impl RefUnwindSafe for MissingRepositoryError {}

/// Signals that an online command has no caller-supplied GitHub credential.
#[ohno::error]
#[display("Neither GITHUB_TOKEN nor GH_TOKEN supplies a GitHub token")]
pub(crate) struct MissingTokenError;

impl UnwindSafe for MissingTokenError {}
impl RefUnwindSafe for MissingTokenError {}

/// Retains the selected report path when local artifact loading fails.
#[ohno::error]
#[display("Failed to read report input from '{}'", path.display())]
pub(crate) struct ReadBodyError {
    path: PathBuf,
}

impl UnwindSafe for ReadBodyError {}
impl RefUnwindSafe for ReadBodyError {}

/// Adds the attempted semantic operation to transport or request-construction failures.
#[ohno::error]
#[display("GitHub request failed while {operation}")]
pub(crate) struct RequestFailedError {
    operation: String,
}

impl UnwindSafe for RequestFailedError {}
impl RefUnwindSafe for RequestFailedError {}

/// Preserves the terminal HTTP status and redacted response for caller-supported decisions.
#[ohno::error]
#[display("GitHub returned HTTP {status} while {operation}: {body}")]
pub(crate) struct UnexpectedStatusError {
    operation: String,
    status: u16,
    body: String,
}

impl UnwindSafe for UnexpectedStatusError {}
impl RefUnwindSafe for UnexpectedStatusError {}

impl UnexpectedStatusError {
    /// Supports the comparison adapter's narrow not-found decision without flattening the error.
    pub(crate) fn status(&self) -> u16 {
        self.status
    }
}

/// Rejects responses whose representation cannot support the requested semantic operation.
#[ohno::error]
#[display("GitHub returned an invalid response while {operation}")]
pub(crate) struct InvalidResponseError {
    operation: String,
}

impl UnwindSafe for InvalidResponseError {}
impl RefUnwindSafe for InvalidResponseError {}

/// Keeps incomplete create responses from being treated as confirmed publication.
#[ohno::error]
#[display("GitHub did not return the created artifact while {operation}")]
pub(crate) struct MissingCreatedArtifactError {
    operation: String,
}

impl UnwindSafe for MissingCreatedArtifactError {}
impl RefUnwindSafe for MissingCreatedArtifactError {}

/// Represents a create whose intended artifact could not be confirmed by reconciliation.
#[ohno::error]
#[display("GitHub create remained ambiguous after marker reconciliation")]
pub(crate) struct AmbiguousCreateError;

impl UnwindSafe for AmbiguousCreateError {}
impl RefUnwindSafe for AmbiguousCreateError {}

/// Attaches report-path context without flattening the original filesystem error.
pub(crate) fn read_body_error(path: PathBuf, error: io::Error) -> ohno::AppError {
    ReadBodyError::caused_by(path, error).into()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use reqwest::StatusCode;

    use super::*;

    #[test]
    fn unexpected_status_exposes_its_status_for_comparison_fallback() {
        let not_found = StatusCode::NOT_FOUND.as_u16();
        let error = UnexpectedStatusError::new("comparing commits", not_found, "missing");
        assert_eq!(error.status(), not_found);
    }
}
