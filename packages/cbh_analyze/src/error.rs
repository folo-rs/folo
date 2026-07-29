//! The error the `analyze`-family commands fail with, [`AnalyzeError`], and the
//! concrete failures it carries.

use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};

use cbh_config::ConfigError;
use cbh_storage::StorageError;
use ohno::OhnoCore;

/// An error from an `analyze`-family command (`analyze`, `list`, `prune`,
/// `examine`, `bless`, `unbless`).
///
/// It is transparent: it renders exactly the message of the concrete failure it
/// carries and adds nothing of its own, so converting one into the binary's
/// [`AppError`](ohno::AppError) leaves the message a user sees unchanged. A caller
/// that must tell the failures apart reaches for the concrete type with
/// [`find_source`](ohno::ErrorExt::find_source).
#[ohno::error]
#[no_constructors]
#[from(ConfigError, StorageError, AnalysisFailedError, BlessFailedError)]
#[from(
    ResolveRefFailedError,
    FirstParentWalkFailedError,
    MergeBaseFailedError
)]
#[from(WorkingTreeProbeFailedError, CommitterTimeFailedError)]
#[from(DefaultBranchProbeFailedError, ToolchainProbeFailedError)]
pub struct AnalyzeError;

// Every error type in this file holds an OhnoCore field containing
// Arc<dyn Error + Send + Sync>, which is !UnwindSafe because Arc requires
// T: RefUnwindSafe and trait objects are !RefUnwindSafe. However, ohno error types are
// immutable after construction — no &self method mutates internal state — so observing
// them through a shared reference during unwind is harmless. That is the reasoning the
// manual impls following each type below rest on.
impl UnwindSafe for AnalyzeError {}
impl RefUnwindSafe for AnalyzeError {}

/// Analyzing stored history failed: a bad filter, a malformed stored object, or no
/// output selected.
#[ohno::error]
#[display("failed to analyze history: {message}")]
pub struct AnalysisFailedError {
    message: String,
}

impl UnwindSafe for AnalysisFailedError {}
impl RefUnwindSafe for AnalysisFailedError {}

/// A blessing precondition failed.
///
/// The causes are: no benchmark prefixes given (and no `--all`), an unresolvable
/// context ref, an undeterminable base branch, or — when the commit has no stored
/// run — an unconstrained target triple or machine key that leaves no discriminant
/// set to synthesize a sidecar for.
#[derive(ohno::Error)]
// Nothing raises a blessing failure from an underlying error, so the generated
// `caused_by` would be dead code.
#[no_constructors]
#[display("blessing failed: {message}")]
pub struct BlessFailedError {
    message: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for BlessFailedError {}
impl RefUnwindSafe for BlessFailedError {}

impl BlessFailedError {
    /// Creates a blessing failure described by `message`.
    #[must_use]
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            core: OhnoCore::default(),
        }
    }
}

/// Asking git what commit a ref names failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to resolve the git ref {reference}")]
pub(crate) struct ResolveRefFailedError {
    reference: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for ResolveRefFailedError {}
impl RefUnwindSafe for ResolveRefFailedError {}

impl ResolveRefFailedError {
    /// Records that resolving `reference` failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(
        reference: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            reference: reference.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Walking a commit's first-parent ancestry failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to walk the first-parent ancestry of {reference}")]
pub(crate) struct FirstParentWalkFailedError {
    reference: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for FirstParentWalkFailedError {}
impl RefUnwindSafe for FirstParentWalkFailedError {}

impl FirstParentWalkFailedError {
    /// Records that walking the ancestry of `reference` failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(
        reference: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            reference: reference.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Asking git for the merge-base of the target and base commits failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to determine the merge-base of {target} and {base}")]
pub(crate) struct MergeBaseFailedError {
    target: String,
    base: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for MergeBaseFailedError {}
impl RefUnwindSafe for MergeBaseFailedError {}

impl MergeBaseFailedError {
    /// Records that the merge-base lookup for `target` and `base` failed because of
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        target: impl Into<String>,
        base: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            target: target.into(),
            base: base.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Asking git for a commit's committer time failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to read the committer time of {reference}")]
pub(crate) struct CommitterTimeFailedError {
    reference: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for CommitterTimeFailedError {}
impl RefUnwindSafe for CommitterTimeFailedError {}

impl CommitterTimeFailedError {
    /// Records that reading the committer time of `reference` failed because of
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        reference: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            reference: reference.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Checking whether the working tree has uncommitted changes failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to check whether the working tree is dirty")]
pub(crate) struct WorkingTreeProbeFailedError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for WorkingTreeProbeFailedError {}
impl RefUnwindSafe for WorkingTreeProbeFailedError {}

impl WorkingTreeProbeFailedError {
    /// Records that the working-tree check failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

/// Detecting the repository's default branch failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to detect the repository's default branch")]
pub(crate) struct DefaultBranchProbeFailedError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for DefaultBranchProbeFailedError {}
impl RefUnwindSafe for DefaultBranchProbeFailedError {}

impl DefaultBranchProbeFailedError {
    /// Records that the default-branch lookup failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

/// Probing the Rust toolchain a run is attributed to failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to probe the Rust toolchain")]
pub(crate) struct ToolchainProbeFailedError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for ToolchainProbeFailedError {}
impl RefUnwindSafe for ToolchainProbeFailedError {}

impl ToolchainProbeFailedError {
    /// Records that the toolchain probe failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::io;

    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(AnalyzeError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(AnalysisFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(BlessFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ResolveRefFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        FirstParentWalkFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(MergeBaseFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        CommitterTimeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        WorkingTreeProbeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        DefaultBranchProbeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ToolchainProbeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );

    #[test]
    fn config_error_passes_through_unchanged() {
        // The wrapper adds no wording of its own, so a configuration failure reads
        // exactly as the configuration layer worded it, with no category prefix.
        let error = AnalyzeError::from(ConfigError::new("bad configuration"));

        assert!(error.message().starts_with("bad configuration"));
        assert!(error.find_source::<ConfigError>().is_some());
    }

    #[test]
    fn storage_error_passes_through_unchanged() {
        // Anchored at the start because the contract under test is that the wrapper
        // prepends nothing; the expected text comes from the storage layer itself
        // rather than being restated here.
        let error = AnalyzeError::from(StorageError::not_found("k"));

        assert!(
            error
                .message()
                .starts_with(&StorageError::not_found("k").message())
        );
        assert!(error.find_source::<StorageError>().is_some());
    }

    #[test]
    fn analysis_failure_is_found_by_type_and_keeps_its_message() {
        let error = AnalyzeError::from(AnalysisFailedError::new("unknown report format"));

        let found = error.find_source::<AnalysisFailedError>().unwrap();
        assert!(found.message().contains("failed to analyze history"));
        assert!(found.message().contains("unknown report format"));
        assert!(found.source().is_none());
    }

    #[test]
    fn analysis_failure_keeps_its_cause() {
        let error = AnalyzeError::from(AnalysisFailedError::caused_by(
            "bad object",
            io::Error::other("not valid UTF-8"),
        ));

        assert!(error.find_source::<AnalysisFailedError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn bless_failure_is_found_by_type_and_keeps_its_message() {
        let error = AnalyzeError::from(BlessFailedError::new("a bless precondition failed"));

        let found = error.find_source::<BlessFailedError>().unwrap();
        assert!(found.message().contains("blessing failed"));
        assert!(found.message().contains("a bless precondition failed"));
        assert!(found.source().is_none());
    }

    #[test]
    fn merge_base_failure_names_the_target_before_the_base() {
        // The two refs read differently in each position, so their order in the
        // rendered message is what tells the user which side was which.
        let error = AnalyzeError::from(MergeBaseFailedError::caused_by(
            "feature",
            "master",
            io::Error::other("shallow clone"),
        ));

        let message = error.message();
        let target = message.find("feature").unwrap();
        let base = message.find("master").unwrap();
        assert!(target < base);
    }

    #[test]
    fn every_git_and_probe_failure_names_a_distinct_operation() {
        // A bare io::Error says only what the operating system reported and reads the
        // same whichever query produced it, so each wrapper's own wording is the only
        // thing that tells the failures apart.
        let cause = || io::Error::other("git is not installed");
        let errors = [
            AnalyzeError::from(ResolveRefFailedError::caused_by("a-ref", cause())),
            AnalyzeError::from(FirstParentWalkFailedError::caused_by("b-ref", cause())),
            AnalyzeError::from(MergeBaseFailedError::caused_by("c-ref", "d-ref", cause())),
            AnalyzeError::from(CommitterTimeFailedError::caused_by("e-ref", cause())),
            AnalyzeError::from(WorkingTreeProbeFailedError::caused_by(cause())),
            AnalyzeError::from(DefaultBranchProbeFailedError::caused_by(cause())),
            AnalyzeError::from(ToolchainProbeFailedError::caused_by(cause())),
        ];

        // The first line is the wrapper's own wording; the cause and any backtrace
        // follow it.
        let headlines = errors
            .iter()
            .map(|error| error.message().lines().next().unwrap().to_owned())
            .collect::<Vec<_>>();

        for (error, headline) in errors.iter().zip(&headlines) {
            assert!(error.message().contains("git is not installed"));
            assert!(error.find_source::<io::Error>().is_some());
            assert_eq!(
                headlines.iter().filter(|other| *other == headline).count(),
                1
            );
        }

        // Each ref-scoped wrapper names the ref it was asked about.
        for (headline, reference) in headlines.iter().zip(["a-ref", "b-ref", "c-ref", "e-ref"]) {
            assert!(headline.contains(reference));
        }
    }
}
