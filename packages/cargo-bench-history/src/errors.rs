//! Private failures reported through [`AppError`](ohno::AppError).

use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use ohno::{AppError, OhnoCore};

// Every error type in this file holds an OhnoCore field containing
// Arc<dyn Error + Send + Sync>, which is !UnwindSafe because Arc requires
// T: RefUnwindSafe and trait objects are !RefUnwindSafe. However, ohno error types are
// immutable after construction — no &self method mutates internal state — so observing
// them through a shared reference during unwind is harmless. That is the reasoning the
// manual impls following each type below rest on.

/// A recoverable per-commit failure retained in an [`AppError`] source chain.
///
/// Backfill uses this typed view both to distinguish recoverable benchmark
/// failures from infrastructure failures and to render the former without
/// flattening their source chains into strings.
#[derive(Debug)]
pub(crate) enum BenchFailure<'a> {
    /// The benchmark command exited unsuccessfully.
    EngineFailed(&'a EngineFailedError),
    /// The benchmark command terminated without an exit code.
    EngineTerminated(&'a EngineTerminatedError),
    /// The benchmark command could not be assembled.
    InvalidCommand(&'a InvalidCommandError),
    /// A harvested benchmark output could not be parsed.
    ParseOutput(&'a ParseOutputError),
}

impl<'a> BenchFailure<'a> {
    /// Finds a recoverable per-commit failure anywhere in `error`'s source chain.
    pub(crate) fn find(error: &'a AppError) -> Option<Self> {
        error
            .find_source::<EngineFailedError>()
            .map(Self::EngineFailed)
            .or_else(|| {
                error
                    .find_source::<EngineTerminatedError>()
                    .map(Self::EngineTerminated)
            })
            .or_else(|| {
                error
                    .find_source::<InvalidCommandError>()
                    .map(Self::InvalidCommand)
            })
            .or_else(|| {
                error
                    .find_source::<ParseOutputError>()
                    .map(Self::ParseOutput)
            })
    }

    /// Renders the failure as one summary line from its structured fields.
    pub(crate) fn render(&self) -> String {
        match self {
            Self::EngineFailed(failure) => format!(
                "engine {:?} failed with exit code {}",
                failure.engine, failure.code
            ),
            Self::EngineTerminated(failure) => {
                format!(
                    "engine {:?} terminated without an exit code",
                    failure.engine
                )
            }
            Self::InvalidCommand(failure) => format!(
                "engine {:?} has an invalid command: {}",
                failure.engine, failure.message
            ),
            Self::ParseOutput(failure) => {
                format!(
                    "failed to parse benchmark output: {}",
                    failure.path.display()
                )
            }
        }
    }
}

/// The benchmark command exited with a non-zero status.
#[derive(ohno::Error)]
#[display("engine {engine:?} failed with exit code {code}")]
pub(crate) struct EngineFailedError {
    engine: String,
    code: i32,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for EngineFailedError {}
impl RefUnwindSafe for EngineFailedError {}

impl EngineFailedError {
    /// The benchmark command that failed (`cargo bench`).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn engine(&self) -> &str {
        &self.engine
    }

    /// The process exit code the command reported.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn code(&self) -> i32 {
        self.code
    }
}

/// The benchmark command was terminated before it could report an exit code.
#[derive(ohno::Error)]
#[no_constructors]
#[display("engine {engine:?} terminated without an exit code")]
pub(crate) struct EngineTerminatedError {
    engine: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for EngineTerminatedError {}
impl RefUnwindSafe for EngineTerminatedError {}

impl EngineTerminatedError {
    /// Records that `engine` terminated without reporting an exit code.
    #[must_use]
    pub(crate) fn new(engine: impl Into<String>) -> Self {
        Self {
            engine: engine.into(),
            core: OhnoCore::default(),
        }
    }

    /// The benchmark command that terminated (`cargo bench`).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn engine(&self) -> &str {
        &self.engine
    }
}

/// The benchmark command could not be assembled into an argv.
#[derive(ohno::Error)]
#[no_constructors]
#[display("engine {engine:?} has an invalid command: {message}")]
pub(crate) struct InvalidCommandError {
    engine: String,
    message: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for InvalidCommandError {}
impl RefUnwindSafe for InvalidCommandError {}

impl InvalidCommandError {
    /// Records that the command configured for `engine` is unusable, as described
    /// by `message`.
    #[must_use]
    pub(crate) fn new(engine: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            engine: engine.into(),
            message: message.into(),
            core: OhnoCore::default(),
        }
    }

    /// The benchmark command whose argv was invalid (`cargo bench`).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn engine(&self) -> &str {
        &self.engine
    }

    /// The reason the command could not be assembled.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn problem(&self) -> &str {
        &self.message
    }
}

/// A harvested benchmark summary could not be parsed.
///
/// The path names the output that could not be read; the parse failure itself is
/// carried as the error's source.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to parse benchmark output: {}", path.display())]
pub(crate) struct ParseOutputError {
    path: PathBuf,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for ParseOutputError {}
impl RefUnwindSafe for ParseOutputError {}

impl ParseOutputError {
    /// Records that the benchmark output at `path` could not be parsed because of
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        path: impl Into<PathBuf>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            path: path.into(),
            core: OhnoCore::from(error),
        }
    }

    /// The benchmark output that could not be parsed.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// A `--best-of` run produced inconsistent results across its repetitions.
///
/// A mismatch makes the per-metric minimum ill-defined.
///
/// The cross-run mismatch is carried as the error's source.
#[derive(ohno::Error)]
#[no_constructors]
#[display("engine {engine:?} produced inconsistent results across --best-of runs")]
pub(crate) struct InconsistentRunsError {
    engine: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for InconsistentRunsError {}
impl RefUnwindSafe for InconsistentRunsError {}

impl InconsistentRunsError {
    /// Records that the repeated harvests of `engine` disagreed, as described by
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        engine: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            engine: engine.into(),
            core: OhnoCore::from(error),
        }
    }

    /// The engine whose repeated harvests disagreed.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn engine(&self) -> &str {
        &self.engine
    }
}

/// A result already exists at this run's computed storage key.
///
/// The run did not request an overwrite.
#[derive(ohno::Error)]
#[display("a result is already stored for this run at {key}; pass --overwrite to replace it")]
pub(crate) struct DuplicateResultError {
    key: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for DuplicateResultError {}
impl RefUnwindSafe for DuplicateResultError {}

impl DuplicateResultError {
    /// The object key that already held a result.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

/// A backfill precondition or per-commit operation failed.
///
/// This includes a dirty working tree, an unresolvable or out-of-history commit
/// range, or stopping after a per-commit failure without `--ignore-errors`.
#[derive(ohno::Error)]
#[display("{message}")]
pub(crate) struct BackfillError {
    message: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for BackfillError {}
impl RefUnwindSafe for BackfillError {}

impl BackfillError {
    /// The explanation and any partial backfill summary.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn summary(&self) -> &str {
        &self.message
    }
}

/// An `import` precondition failed (for example a `--commit` override that resolves
/// to no commit in the repository).
#[derive(ohno::Error)]
#[no_constructors]
#[display("{message}")]
pub(crate) struct ImportError {
    message: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for ImportError {}
impl RefUnwindSafe for ImportError {}

impl ImportError {
    /// Records an import precondition failure described by `message`.
    #[must_use]
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            core: OhnoCore::default(),
        }
    }

    /// The reason the import precondition failed.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn reason(&self) -> &str {
        &self.message
    }
}

/// Asking the operating system for the process working directory failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to determine the process working directory")]
pub(crate) struct WorkingDirectoryFailedError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for WorkingDirectoryFailedError {}
impl RefUnwindSafe for WorkingDirectoryFailedError {}

impl WorkingDirectoryFailedError {
    /// Records that the working-directory lookup failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

/// Spawning or waiting for the benchmark command failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to run the benchmark command {command:?}")]
pub(crate) struct BenchCommandFailedError {
    command: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for BenchCommandFailedError {}
impl RefUnwindSafe for BenchCommandFailedError {}

impl BenchCommandFailedError {
    /// Records that running `command` failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(
        command: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            command: command.into(),
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

/// Probing the repository's git state a run is attributed to failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to probe the repository's git state")]
pub(crate) struct GitProbeFailedError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for GitProbeFailedError {}
impl RefUnwindSafe for GitProbeFailedError {}

impl GitProbeFailedError {
    /// Records that the git probe failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

/// Reading the benchmark output an engine left behind failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to harvest {engine} benchmark output")]
pub(crate) struct HarvestFailedError {
    engine: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for HarvestFailedError {}
impl RefUnwindSafe for HarvestFailedError {}

impl HarvestFailedError {
    /// Records that harvesting the output of `engine` failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(
        engine: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            engine: engine.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Writing a requested report file failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to write the {label} report to {}", path.display())]
pub(crate) struct WriteReportFailedError {
    pub(crate) label: String,
    pub(crate) path: PathBuf,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for WriteReportFailedError {}
impl RefUnwindSafe for WriteReportFailedError {}

impl WriteReportFailedError {
    /// Records that writing the `label` report to `path` failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(
        label: impl Into<String>,
        path: impl Into<PathBuf>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Writing the configuration file `install` generates failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to write the configuration file to {}", path.display())]
pub(crate) struct WriteConfigFailedError {
    path: PathBuf,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for WriteConfigFailedError {}
impl RefUnwindSafe for WriteConfigFailedError {}

impl WriteConfigFailedError {
    /// Records that writing the configuration file to `path` failed because of
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        path: impl Into<PathBuf>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            path: path.into(),
            core: OhnoCore::from(error),
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

/// Creating the scratch git worktree a backfill checks each commit out into
/// failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to add a git worktree at {} for commit {commit}", path.display())]
pub(crate) struct AddWorktreeFailedError {
    path: PathBuf,
    commit: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for AddWorktreeFailedError {}
impl RefUnwindSafe for AddWorktreeFailedError {}

impl AddWorktreeFailedError {
    /// Records that adding a worktree at `path` for `commit` failed because of
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        path: impl Into<PathBuf>,
        commit: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            path: path.into(),
            commit: commit.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Checking the next commit out into the scratch git worktree failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to reset the git worktree at {} to commit {commit}", path.display())]
pub(crate) struct ResetWorktreeFailedError {
    path: PathBuf,
    commit: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for ResetWorktreeFailedError {}
impl RefUnwindSafe for ResetWorktreeFailedError {}

impl ResetWorktreeFailedError {
    /// Records that resetting the worktree at `path` to `commit` failed because of
    /// `error`.
    #[must_use]
    pub(crate) fn caused_by(
        path: impl Into<PathBuf>,
        commit: impl Into<String>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            path: path.into(),
            commit: commit.into(),
            core: OhnoCore::from(error),
        }
    }
}

/// Tearing the scratch git worktree down after a backfill failed.
#[derive(ohno::Error)]
#[no_constructors]
#[display("failed to remove the git worktree at {}", path.display())]
pub(crate) struct RemoveWorktreeFailedError {
    path: PathBuf,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for RemoveWorktreeFailedError {}
impl RefUnwindSafe for RemoveWorktreeFailedError {}

impl RemoveWorktreeFailedError {
    /// Records that removing the worktree at `path` failed because of `error`.
    #[must_use]
    pub(crate) fn caused_by(
        path: impl Into<PathBuf>,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        Self {
            path: path.into(),
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

    assert_impl_all!(EngineFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        EngineTerminatedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(InvalidCommandError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ParseOutputError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        InconsistentRunsError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        DuplicateResultError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(BackfillError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ImportError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        WorkingDirectoryFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        BenchCommandFailedError: Send,
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
    assert_impl_all!(GitProbeFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(HarvestFailedError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        WriteReportFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        WriteConfigFailedError: Send,
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
    assert_impl_all!(
        AddWorktreeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ResetWorktreeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        RemoveWorktreeFailedError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );

    #[test]
    fn engine_failure_names_the_engine_and_the_exit_code() {
        let error = EngineFailedError::new("callgrind", 101);

        assert_eq!(error.engine(), "callgrind");
        assert_eq!(error.code(), 101);
        assert!(error.source().is_none());
    }

    #[test]
    fn engine_termination_carries_the_engine() {
        let terminated = EngineTerminatedError::new("callgrind");

        assert_eq!(terminated.engine(), "callgrind");
        assert!(terminated.source().is_none());
    }

    #[test]
    fn invalid_command_names_the_engine_and_the_problem() {
        let error = InvalidCommandError::new("cargo bench", "command is empty");

        assert_eq!(error.engine(), "cargo bench");
        assert_eq!(error.problem(), "command is empty");
    }

    #[test]
    fn parse_failure_carries_the_output_path_and_source() {
        let error = ParseOutputError::caused_by(
            "target/callgrind/summary.json",
            io::Error::other("missing 'value' field"),
        );

        assert_eq!(error.path(), Path::new("target/callgrind/summary.json"));
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn inconsistent_runs_carry_the_engine_and_mismatch_source() {
        let error = InconsistentRunsError::caused_by(
            "criterion",
            io::Error::other("case 'a/b' is missing from run 2"),
        );

        assert_eq!(error.engine(), "criterion");
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn duplicate_result_carries_the_conflicting_key() {
        let error = DuplicateResultError::new("v1/folo/objects/callgrind/t/m1/abc/clean.json");

        assert_eq!(error.key(), "v1/folo/objects/callgrind/t/m1/abc/clean.json");
    }

    #[test]
    fn backfill_and_import_failures_carry_their_explanations() {
        let backfill = BackfillError::new("the working tree is dirty");
        let import = ImportError::new("--commit resolves to no commit in the repository");

        assert_eq!(backfill.summary(), "the working tree is dirty");
        assert_eq!(
            import.reason(),
            "--commit resolves to no commit in the repository"
        );
    }

    #[test]
    fn working_directory_failure_carries_its_source() {
        let error =
            WorkingDirectoryFailedError::caused_by(io::Error::other("working directory missing"));

        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn bench_failures_are_the_four_per_commit_failures() {
        let bench: [AppError; 4] = [
            EngineFailedError::new("cargo bench", 101).into(),
            EngineTerminatedError::new("cargo bench").into(),
            InvalidCommandError::new("cargo bench", "command is empty").into(),
            ParseOutputError::caused_by("a/summary.json", io::Error::other("bad")).into(),
        ];

        for error in &bench {
            assert!(BenchFailure::find(error).is_some());
        }
    }

    #[test]
    fn bench_failure_summaries_render_the_typed_fields() {
        let failed = AppError::from(EngineFailedError::new("callgrind", 101));
        let terminated = AppError::from(EngineTerminatedError::new("criterion"));
        let invalid = AppError::from(InvalidCommandError::new("cargo bench", "command is empty"));
        let parse = AppError::from(ParseOutputError::caused_by(
            "target/all-the-time/results.json",
            io::Error::other("invalid document"),
        ));

        let failed = BenchFailure::find(&failed).unwrap().render();
        assert!(failed.contains("callgrind"));
        assert!(failed.contains("101"));

        let terminated = BenchFailure::find(&terminated).unwrap().render();
        assert!(terminated.contains("criterion"));

        let invalid = BenchFailure::find(&invalid).unwrap().render();
        assert!(invalid.contains("cargo bench"));
        assert!(invalid.contains("command is empty"));

        let parse = BenchFailure::find(&parse).unwrap().render();
        assert!(parse.contains("target/all-the-time/results.json"));
    }

    #[test]
    fn infrastructure_failures_are_not_bench_failures() {
        // A backfill must abort on these rather than record them and continue, so
        // misclassifying one would let a run finish with incorrect data.
        let infrastructure: [AppError; 4] = [
            DuplicateResultError::new("v1/p/objects/e/t/m/abc/clean.json").into(),
            InconsistentRunsError::caused_by("cargo bench", io::Error::other("a/b is missing"))
                .into(),
            AddWorktreeFailedError::caused_by("a/tree", "c0ffee", io::Error::other("no space"))
                .into(),
            io::Error::other("access denied").into(),
        ];

        for error in &infrastructure {
            assert!(BenchFailure::find(error).is_none());
        }
    }

    #[test]
    fn a_bench_failure_is_recognized_anywhere_in_the_chain() {
        // Classification must follow the whole chain: a bench failure reported as the
        // cause of some outer failure is still a bench failure.
        let nested = AppError::from(HarvestFailedError::caused_by(
            "criterion",
            ParseOutputError::caused_by("a/summary.json", io::Error::other("bad")),
        ));

        let Some(BenchFailure::ParseOutput(failure)) = BenchFailure::find(&nested) else {
            panic!("expected the nested parse failure");
        };
        assert_eq!(failure.path, Path::new("a/summary.json"));
        assert!(failure.find_source::<io::Error>().is_some());
    }
}
