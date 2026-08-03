//! The error the `analyze`-family commands fail with, [`AnalyzeError`], and the
//! concrete failures it carries.

use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};

use cbh_config::ConfigError;
use cbh_model::EmptyBenchmarkIdPrefix;
use cbh_storage::StorageError;
use ohno::OhnoCore;

/// An error from an `analyze`-family command (`analyze`, `list`, `prune`,
/// `examine`, `bless`, `unbless`).
///
/// It is transparent: it renders exactly the diagnostic of the failed operation
/// and adds nothing when converted into the application's
/// [`AppError`](ohno::AppError).
#[ohno::error]
#[no_constructors]
#[from(ConfigError, StorageError, NoOutputSelectedError)]
#[from(UnknownEngineError, EmptyBenchmarkSelectionError, UnknownMetricError)]
#[from(InvalidListWideningError, PruneSelectionRequiredError)]
#[from(PruneBaseConfirmationRequiredError, BaseBranchUnavailableError)]
#[from(MergeBaseUnavailableError, RefNotFoundError)]
#[from(StoredObjectUtf8Error, InvalidResultSetError, InvalidBlessingError)]
#[from(InvalidWindowValueError, WindowOutOfRangeError)]
#[from(
    BlessSelectionRequiredError,
    BlessBaseRequiredError,
    BlessDiscriminantsRequiredError
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

/// A reporting command was configured to produce no output.
#[ohno::error]
#[display("no output selected: {guidance}")]
pub(crate) struct NoOutputSelectedError {
    guidance: String,
}

impl UnwindSafe for NoOutputSelectedError {}
impl RefUnwindSafe for NoOutputSelectedError {}

/// A requested benchmark engine name is not supported.
#[ohno::error]
#[display(
    "unknown engine {name:?}; expected one of: criterion, callgrind, alloc_tracker, all_the_time"
)]
pub(crate) struct UnknownEngineError {
    name: String,
}

/// `examine` received an empty benchmark selection.
#[ohno::error]
#[display("--benchmark must not be empty")]
#[from(EmptyBenchmarkIdPrefix)]
pub(crate) struct EmptyBenchmarkSelectionError;

/// A requested metric name is not supported.
#[ohno::error]
#[display("unknown metric {name:?}; expected one of: {valid}")]
pub(crate) struct UnknownMetricError {
    name: String,
    valid: String,
}

/// `--all` was applied to a list subject that cannot be widened.
#[ohno::error]
#[display(
    "--all applies only to `list blessings`, where it widens the view from the current commit to \
     the most recent blessing of every benchmark in the window; it has no meaning for `list runs` \
     or `list discriminants`"
)]
pub(crate) struct InvalidListWideningError;

/// A prune command did not select any object category.
#[ohno::error]
#[display(
    "specify what to delete: --clean (clean runs), --dirty (dirty snapshots), --all (both), and/or \
     --include-blessings (blessing sidecars)"
)]
pub(crate) struct PruneSelectionRequiredError;

/// Pruning base-branch history requires explicit confirmation.
#[ohno::error]
#[display(
    "this will delete benchmark history of the {base_name} branch, which is the base branch. \
     Confirm with --prune-base if this is correct."
)]
pub(crate) struct PruneBaseConfirmationRequiredError {
    base_name: String,
}

/// No default base branch was available for a comparison.
#[ohno::error]
#[display(
    "could not determine the base branch to compare {target_ref} against: no --base was given and \
     no default branch could be resolved. Pass an explicit --base, set project.default_branch, or \
     make the default branch available as origin/HEAD."
)]
pub(crate) struct BaseBranchUnavailableError {
    target_ref: String,
}

/// The target and base commits have no available common ancestor.
#[ohno::error]
#[display(
    "could not determine the merge-base of the target {target_ref} ({target_commit}) and the base \
     commit {base_commit}: they share no common ancestor in the available history. {guidance}"
)]
pub(crate) struct MergeBaseUnavailableError {
    target_ref: String,
    target_commit: String,
    base_commit: String,
    guidance: String,
}

/// A requested git reference could not be resolved.
#[ohno::error]
#[display("could not resolve {reference:?}. {guidance}")]
pub(crate) struct RefNotFoundError {
    reference: String,
    guidance: String,
}

/// A stored object contains bytes that are not valid UTF-8.
#[ohno::error]
#[display("stored {object_kind} {key} is not valid UTF-8")]
pub(crate) struct StoredObjectUtf8Error {
    object_kind: &'static str,
    key: String,
}

/// A stored result object does not match the result-set schema.
#[ohno::error]
#[display("stored object {key} is not a valid result set")]
pub(crate) struct InvalidResultSetError {
    key: String,
}

/// A stored blessing object does not match the blessing schema.
#[ohno::error]
#[display("stored blessing {key} is not a valid blessing record")]
pub(crate) struct InvalidBlessingError {
    key: String,
}

/// A time-window option has no supported timestamp, date, or duration syntax.
#[ohno::error]
#[display(
    "invalid {flag} value {value:?}; expected an RFC 3339 timestamp, a YYYY-MM-DD date, or a \
     relative duration such as \"6 months\" or \"30 days ago\""
)]
pub(crate) struct InvalidWindowValueError {
    flag: String,
    value: String,
}

/// Time-window arithmetic exceeded the representable timestamp range.
#[ohno::error]
#[display("{context} is out of the representable range")]
pub(crate) struct WindowOutOfRangeError {
    context: String,
}

macro_rules! impl_error_unwind {
    ($($error:ty),+ $(,)?) => {
        $(
            impl UnwindSafe for $error {}
            impl RefUnwindSafe for $error {}
        )+
    };
}

impl_error_unwind!(
    UnknownEngineError,
    EmptyBenchmarkSelectionError,
    UnknownMetricError,
    InvalidListWideningError,
    PruneSelectionRequiredError,
    PruneBaseConfirmationRequiredError,
    BaseBranchUnavailableError,
    MergeBaseUnavailableError,
    RefNotFoundError,
    StoredObjectUtf8Error,
    InvalidResultSetError,
    InvalidBlessingError,
    InvalidWindowValueError,
    WindowOutOfRangeError,
);

/// No benchmark selection was provided for `bless`.
#[ohno::error]
#[display(
    "at least one benchmark-id prefix is required (or pass --all); for example `bless \
     all_the_time/read_cell`"
)]
pub(crate) struct BlessSelectionRequiredError;

impl UnwindSafe for BlessSelectionRequiredError {}
impl RefUnwindSafe for BlessSelectionRequiredError {}

/// The base branch for a blessing could not be determined.
#[ohno::error]
#[display("could not determine the base branch; specify it with --base")]
pub(crate) struct BlessBaseRequiredError;

impl UnwindSafe for BlessBaseRequiredError {}
impl RefUnwindSafe for BlessBaseRequiredError {}

/// A blessing could not identify a concrete discriminant set.
#[ohno::error]
#[display(
    "no stored result at the context commit {commit} and the target-triple or machine-key facet is \
     unconstrained, so no discriminant set can be targeted; pass --target-triple and --machine-key \
     (or record a run at the commit first)"
)]
pub(crate) struct BlessDiscriminantsRequiredError {
    commit: String,
}

impl UnwindSafe for BlessDiscriminantsRequiredError {}
impl RefUnwindSafe for BlessDiscriminantsRequiredError {}

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

    use cbh_storage::{MemoryStorage, Storage as _};
    use futures::executor::block_on;
    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(AnalyzeError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NoOutputSelectedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RefNotFoundError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(StoredObjectUtf8Error: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
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
        let configuration = cbh_config::parse_config("[").unwrap_err();
        let expected = configuration.message();
        let error = AnalyzeError::from(configuration);

        assert!(error.message().starts_with(&expected));
        assert!(error.find_source::<ConfigError>().is_some());
    }

    #[test]
    fn storage_error_passes_through_unchanged() {
        // Anchored at the start because the contract under test is that the wrapper
        // prepends nothing; the expected text comes from the storage layer itself
        // rather than being restated here.
        let storage = MemoryStorage::new();
        let storage_error = block_on(storage.get("k")).unwrap_err();
        let expected = storage_error.message();
        let error = AnalyzeError::from(storage_error);

        assert!(error.message().starts_with(&expected));
        assert!(error.find_source::<StorageError>().is_some());
    }

    #[test]
    fn unresolved_ref_is_found_by_private_type() {
        let error = AnalyzeError::from(RefNotFoundError::new("HEAD", "check the ref"));

        let found = error.find_source::<RefNotFoundError>().unwrap();
        assert!(found.source().is_none());
    }

    #[test]
    fn stored_utf8_failure_keeps_its_cause() {
        let error = AnalyzeError::from(StoredObjectUtf8Error::caused_by(
            "object",
            "v1/result.json",
            io::Error::other("not valid UTF-8"),
        ));

        assert!(error.find_source::<StoredObjectUtf8Error>().is_some());
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
