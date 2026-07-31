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
#[from(ConfigError, StorageError, AnalysisFailedError, NoOutputSelectedError)]
#[from(RepositoryRequiredError)]
#[from(
    BlessSelectionRequiredError,
    BlessBaseRequiredError,
    BlessDiscriminantsRequiredError,
    BlessRefNotFoundError
)]
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

/// An analysis operation could not be completed.
///
/// This includes invalid selections, malformed stored objects, and invalid time
/// windows.
#[ohno::error]
#[display("failed to analyze history: {message}")]
pub struct AnalysisFailedError {
    message: String,
}

impl UnwindSafe for AnalysisFailedError {}
impl RefUnwindSafe for AnalysisFailedError {}

/// A reporting command was configured to produce no output.
#[ohno::error]
#[display("failed to analyze history: no output selected: {guidance}")]
pub struct NoOutputSelectedError {
    guidance: String,
}

impl UnwindSafe for NoOutputSelectedError {}
impl RefUnwindSafe for NoOutputSelectedError {}

/// Repository history required by a command was unavailable.
#[ohno::error]
#[display(
    "failed to analyze history: this command requires a git repository: could not resolve \
     {reference:?}. {guidance}"
)]
pub struct RepositoryRequiredError {
    reference: String,
    guidance: String,
}

impl UnwindSafe for RepositoryRequiredError {}
impl RefUnwindSafe for RepositoryRequiredError {}

/// No benchmark selection was provided for `bless`.
#[ohno::error]
#[display(
    "blessing failed: at least one benchmark-id prefix is required (or pass --all); for example \
     `bless all_the_time/read_cell`"
)]
pub(crate) struct BlessSelectionRequiredError;

impl UnwindSafe for BlessSelectionRequiredError {}
impl RefUnwindSafe for BlessSelectionRequiredError {}

/// The base branch for a blessing could not be determined.
#[ohno::error]
#[display("blessing failed: could not determine the base branch; specify it with --base")]
pub(crate) struct BlessBaseRequiredError;

impl UnwindSafe for BlessBaseRequiredError {}
impl RefUnwindSafe for BlessBaseRequiredError {}

/// A blessing could not identify a concrete discriminant set.
#[ohno::error]
#[display(
    "blessing failed: no stored result at the context commit {commit} and the target-triple or \
     machine-key facet is unconstrained, so no discriminant set can be targeted; pass \
     --target-triple and --machine-key (or record a run at the commit first)"
)]
pub(crate) struct BlessDiscriminantsRequiredError {
    commit: String,
}

impl UnwindSafe for BlessDiscriminantsRequiredError {}
impl RefUnwindSafe for BlessDiscriminantsRequiredError {}

/// The commit to bless could not be resolved.
#[ohno::error]
#[display(
    "blessing failed: could not resolve {reference}; run this inside a git repository (or pass \
     --repo) and check the ref exists"
)]
pub(crate) struct BlessRefNotFoundError {
    reference: String,
}

impl UnwindSafe for BlessRefNotFoundError {}
impl RefUnwindSafe for BlessRefNotFoundError {}

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
    assert_impl_all!(NoOutputSelectedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RepositoryRequiredError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        BlessSelectionRequiredError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        BlessBaseRequiredError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        BlessDiscriminantsRequiredError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        BlessRefNotFoundError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
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
    fn analysis_failure_is_found_by_type() {
        let error = AnalyzeError::from(AnalysisFailedError::new("unknown report format"));

        let found = error.find_source::<AnalysisFailedError>().unwrap();
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
    fn bless_selection_failure_is_found_by_type() {
        let error = AnalyzeError::from(BlessSelectionRequiredError::new());

        let found = error.find_source::<BlessSelectionRequiredError>().unwrap();
        assert!(found.source().is_none());
    }

    #[test]
    fn merge_base_failure_carries_the_target_and_base() {
        let error =
            MergeBaseFailedError::caused_by("feature", "master", io::Error::other("shallow clone"));

        assert_eq!(error.target, "feature");
        assert_eq!(error.base, "master");
        assert!(error.find_source::<io::Error>().is_some());
    }
}
