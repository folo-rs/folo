//! The error the `analyze`-family commands fail with, [`AnalyzeError`].

use std::panic::{RefUnwindSafe, UnwindSafe};

use cbh_config::ConfigError;
use cbh_storage::StorageError;

/// An error from an `analyze`-family command (`analyze`, `list`, `prune`,
/// `examine`, `bless`, `unbless`).
///
/// It is transparent: it renders exactly the message of the failure it carries
/// and adds nothing of its own, so converting one into the binary's
/// [`AppError`](ohno::AppError) leaves the message a user sees unchanged.
#[ohno::error]
#[no_constructors]
#[from(ConfigError, StorageError, NoOutputSelectedError)]
#[from(
    UnknownEngineError,
    EmptyBenchmarkError,
    UnknownMetricError,
    ListAllUnsupportedError
)]
#[from(PruneSelectionRequiredError, PruneBaseConfirmationRequiredError)]
#[from(BaseBranchUnavailableError, MergeBaseUnavailableError)]
#[from(MergeBaseOffFirstParentError)]
#[from(UnresolvedRefError)]
#[from(InvalidStoredUtf8Error, InvalidResultSetError, InvalidBlessingError)]
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

/// An engine selector did not name a supported benchmark engine.
#[ohno::error]
#[display(
    "unknown engine {name:?}; expected one of: criterion, callgrind, alloc_tracker, all_the_time"
)]
pub(crate) struct UnknownEngineError {
    pub(crate) name: String,
}

impl UnwindSafe for UnknownEngineError {}
impl RefUnwindSafe for UnknownEngineError {}

/// The benchmark selector for `examine` was empty.
#[ohno::error]
#[display("--benchmark must not be empty")]
pub(crate) struct EmptyBenchmarkError;

impl UnwindSafe for EmptyBenchmarkError {}
impl RefUnwindSafe for EmptyBenchmarkError {}

/// A metric selector did not name a supported metric.
#[ohno::error]
#[display("unknown metric {name:?}; expected one of: {valid}")]
pub(crate) struct UnknownMetricError {
    pub(crate) name: String,
    pub(crate) valid: String,
}

impl UnwindSafe for UnknownMetricError {}
impl RefUnwindSafe for UnknownMetricError {}

/// The `--all` switch was used for a list subject it cannot widen.
#[ohno::error]
#[display(
    "--all applies only to `list blessings`, where it widens the view from the current commit to \
     the most recent blessing of every benchmark in the window; it has no meaning for `list runs` \
     or `list discriminants`"
)]
pub(crate) struct ListAllUnsupportedError;

impl UnwindSafe for ListAllUnsupportedError {}
impl RefUnwindSafe for ListAllUnsupportedError {}

/// A prune command did not select anything to delete.
#[ohno::error]
#[display(
    "prune requires a deletion scope: --clean (clean runs), --dirty (dirty snapshots), --all \
     (both), and/or --include-blessings (blessing sidecars)"
)]
pub(crate) struct PruneSelectionRequiredError;

impl UnwindSafe for PruneSelectionRequiredError {}
impl RefUnwindSafe for PruneSelectionRequiredError {}

/// Pruning base-branch history was not explicitly confirmed.
#[ohno::error]
#[display(
    "pruning will delete benchmark history of the {base_name} branch, which is the base branch. \
     Confirm with --prune-base if this is correct."
)]
pub(crate) struct PruneBaseConfirmationRequiredError {
    pub(crate) base_name: String,
}

impl UnwindSafe for PruneBaseConfirmationRequiredError {}
impl RefUnwindSafe for PruneBaseConfirmationRequiredError {}

/// No base branch could be selected for a target ref.
#[ohno::error]
#[display(
    "could not determine the base branch to compare {target_ref} against: no --base was given and \
     no default branch could be resolved. Pass an explicit --base, set project.default_branch, or \
     make the default branch available (a shallow clone or a checkout that never fetched the base \
     branch is the usual cause)."
)]
pub(crate) struct BaseBranchUnavailableError {
    pub(crate) target_ref: String,
}

impl UnwindSafe for BaseBranchUnavailableError {}
impl RefUnwindSafe for BaseBranchUnavailableError {}

/// The target and base had no merge-base in the available history.
#[ohno::error]
#[display(
    "could not determine the merge-base of the target {target_ref} ({target_commit}) and the base \
     commit {base_commit}: they share no common ancestor in the available history. This is almost \
     always a shallow clone whose depth stops short of the branch point — fetch the full history \
     (`git fetch --unshallow`, or set fetch-depth: 0 on actions/checkout) so the branch point is \
     present.{remedy}"
)]
pub(crate) struct MergeBaseUnavailableError {
    pub(crate) target_ref: String,
    pub(crate) target_commit: String,
    pub(crate) base_commit: String,
    pub(crate) remedy: String,
}

impl UnwindSafe for MergeBaseUnavailableError {}
impl RefUnwindSafe for MergeBaseUnavailableError {}

/// The merge-base resolved but does not lie on the target's first-parent line.
#[ohno::error]
#[display(
    "the merge-base {merge_base} of the target {target_ref} ({target_commit}) and the base \
     commit {base_commit} is not on the target's first-parent line, so the branch cannot be \
     split from its base and the comparison has no baseline. This happens when the base was \
     merged into the branch instead of the branch being rebased onto the base; rebase the branch \
     onto its base, or analyze a --context whose first-parent history reaches the merge-base."
)]
pub(crate) struct MergeBaseOffFirstParentError {
    pub(crate) target_ref: String,
    pub(crate) target_commit: String,
    pub(crate) base_commit: String,
    pub(crate) merge_base: String,
}

impl UnwindSafe for MergeBaseOffFirstParentError {}
impl RefUnwindSafe for MergeBaseOffFirstParentError {}

/// A git ref did not resolve to a commit.
#[ohno::error]
#[display("could not resolve {reference:?} while {operation}. {guidance}")]
pub(crate) struct UnresolvedRefError {
    pub(crate) operation: String,
    pub(crate) reference: String,
    pub(crate) guidance: String,
}

impl UnwindSafe for UnresolvedRefError {}
impl RefUnwindSafe for UnresolvedRefError {}

/// A stored object's bytes were not valid UTF-8.
#[ohno::error]
#[display("{object_kind} {key} is not valid UTF-8")]
pub(crate) struct InvalidStoredUtf8Error {
    pub(crate) object_kind: String,
    pub(crate) key: String,
}

impl UnwindSafe for InvalidStoredUtf8Error {}
impl RefUnwindSafe for InvalidStoredUtf8Error {}

/// A stored run did not contain a valid result set.
#[ohno::error]
#[display("stored object {key} is not a valid result set")]
pub(crate) struct InvalidResultSetError {
    pub(crate) key: String,
}

impl UnwindSafe for InvalidResultSetError {}
impl RefUnwindSafe for InvalidResultSetError {}

/// A stored blessing did not contain a valid blessing record.
#[ohno::error]
#[display("{object_kind} {key} is not a valid {expected}")]
pub(crate) struct InvalidBlessingError {
    pub(crate) object_kind: String,
    pub(crate) key: String,
    pub(crate) expected: String,
}

impl UnwindSafe for InvalidBlessingError {}
impl RefUnwindSafe for InvalidBlessingError {}

/// A time-window selector was not in any supported syntax.
#[ohno::error]
#[display(
    "invalid {flag} value {value:?}; expected an RFC 3339 timestamp, a YYYY-MM-DD date, or a \
     relative duration such as \"6 months\" or \"30 days ago\""
)]
pub(crate) struct InvalidWindowValueError {
    pub(crate) flag: String,
    pub(crate) value: String,
}

impl UnwindSafe for InvalidWindowValueError {}
impl RefUnwindSafe for InvalidWindowValueError {}

/// A time-window calculation exceeded the timestamp range.
#[ohno::error]
#[display("{window} is out of the representable range")]
pub(crate) struct WindowOutOfRangeError {
    pub(crate) window: String,
}

impl UnwindSafe for WindowOutOfRangeError {}
impl RefUnwindSafe for WindowOutOfRangeError {}

/// A reporting command was configured to produce no output.
#[ohno::error]
#[display("no output selected: {guidance}")]
pub(crate) struct NoOutputSelectedError {
    guidance: String,
}

impl UnwindSafe for NoOutputSelectedError {}
impl RefUnwindSafe for NoOutputSelectedError {}

/// No benchmark selection was provided for `bless`.
#[ohno::error]
#[display(
    "bless requires at least one benchmark-id prefix (or pass --all); for example `bless \
     all_the_time/read_cell`"
)]
pub(crate) struct BlessSelectionRequiredError;

impl UnwindSafe for BlessSelectionRequiredError {}
impl RefUnwindSafe for BlessSelectionRequiredError {}

/// The base branch for a blessing could not be determined.
#[ohno::error]
#[display("bless could not determine the base branch; specify it with --base")]
pub(crate) struct BlessBaseRequiredError;

impl UnwindSafe for BlessBaseRequiredError {}
impl RefUnwindSafe for BlessBaseRequiredError {}

/// A blessing could not identify a concrete discriminant set.
#[ohno::error]
#[display(
    "bless cannot target a discriminant set: no stored result exists at the context commit \
     {commit}, and the target-triple or machine-key filter is unconstrained; pass --target-triple \
     and --machine-key (or record a run at the commit first)"
)]
pub(crate) struct BlessDiscriminantsRequiredError {
    commit: String,
}

impl UnwindSafe for BlessDiscriminantsRequiredError {}
impl RefUnwindSafe for BlessDiscriminantsRequiredError {}

/// Asking git what commit a ref names failed.
#[ohno::error]
#[display("failed to resolve the git ref {reference}")]
pub(crate) struct ResolveRefFailedError {
    reference: String,
}

impl UnwindSafe for ResolveRefFailedError {}
impl RefUnwindSafe for ResolveRefFailedError {}

/// Walking a commit's first-parent ancestry failed.
#[ohno::error]
#[display("failed to walk the first-parent ancestry of {reference}")]
pub(crate) struct FirstParentWalkFailedError {
    reference: String,
}

impl UnwindSafe for FirstParentWalkFailedError {}
impl RefUnwindSafe for FirstParentWalkFailedError {}

/// Asking git for the merge-base of the target and base commits failed.
#[ohno::error]
#[display("failed to determine the merge-base of {target} and {base}")]
pub(crate) struct MergeBaseFailedError {
    target: String,
    base: String,
}

impl UnwindSafe for MergeBaseFailedError {}
impl RefUnwindSafe for MergeBaseFailedError {}

/// Asking git for a commit's committer time failed.
#[ohno::error]
#[display("failed to read the committer time of {reference}")]
pub(crate) struct CommitterTimeFailedError {
    reference: String,
}

impl UnwindSafe for CommitterTimeFailedError {}
impl RefUnwindSafe for CommitterTimeFailedError {}

/// Checking whether the working tree has uncommitted changes failed.
#[ohno::error]
#[display("failed to check whether the working tree is dirty")]
pub(crate) struct WorkingTreeProbeFailedError;

impl UnwindSafe for WorkingTreeProbeFailedError {}
impl RefUnwindSafe for WorkingTreeProbeFailedError {}

/// Detecting the repository's default branch failed.
#[ohno::error]
#[display("failed to detect the repository's default branch")]
pub(crate) struct DefaultBranchProbeFailedError;

impl UnwindSafe for DefaultBranchProbeFailedError {}
impl RefUnwindSafe for DefaultBranchProbeFailedError {}

/// Probing the Rust toolchain a run is attributed to failed.
#[ohno::error]
#[display("failed to probe the Rust toolchain")]
pub(crate) struct ToolchainProbeFailedError;

impl UnwindSafe for ToolchainProbeFailedError {}
impl RefUnwindSafe for ToolchainProbeFailedError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::Debug;
    use std::io;

    use cbh_config::parse_config;
    use cbh_storage::{MemoryStorage, Storage as _};
    use futures::executor::block_on;
    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(AnalyzeError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NoOutputSelectedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnknownEngineError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EmptyBenchmarkError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnknownMetricError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ListAllUnsupportedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PruneSelectionRequiredError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        PruneBaseConfirmationRequiredError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        BaseBranchUnavailableError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        MergeBaseUnavailableError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        MergeBaseOffFirstParentError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(UnresolvedRefError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidStoredUtf8Error: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidResultSetError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidBlessingError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidWindowValueError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WindowOutOfRangeError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
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
        let source = parse_config("invalid = [").unwrap_err();
        let source_message = source.message();
        let error = AnalyzeError::from(source);

        assert_eq!(error.message(), source_message);
        assert!(error.find_source::<ConfigError>().is_some());
    }

    #[test]
    fn storage_error_passes_through_unchanged() {
        let source = block_on(MemoryStorage::new().get("missing")).unwrap_err();
        let source_message = source.message();
        let error = AnalyzeError::from(source);

        assert_eq!(error.message(), source_message);
        assert!(error.find_source::<StorageError>().is_some());
    }

    #[test]
    fn condition_is_found_by_type_with_its_fields() {
        let error = AnalyzeError::from(UnknownMetricError::new("unknown", "wall_time, cpu_time"));

        let found = error.find_source::<UnknownMetricError>().unwrap();
        assert_eq!(found.name, "unknown");
        assert_eq!(found.valid, "wall_time, cpu_time");
        assert!(found.source().is_none());
    }

    #[test]
    fn condition_keeps_its_cause() {
        let error = AnalyzeError::from(InvalidStoredUtf8Error::caused_by(
            "stored object",
            "key",
            io::Error::other("not valid UTF-8"),
        ));

        let found = error.find_source::<InvalidStoredUtf8Error>().unwrap();
        assert_eq!(found.object_kind, "stored object");
        assert_eq!(found.key, "key");
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
