//! The concrete failures `run` and the command handlers report through
//! [`AppError`](ohno::AppError).

use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

use ohno::{AppError, OhnoCore};

// Every error type in this file holds an OhnoCore field containing
// Arc<dyn Error + Send + Sync>, which is !UnwindSafe because Arc requires
// T: RefUnwindSafe and trait objects are !RefUnwindSafe. However, ohno error types are
// immutable after construction — no &self method mutates internal state — so observing
// them through a shared reference during unwind is harmless. That is the reasoning the
// manual impls following each type below rest on.

/// The one-line reason `error` is a per-commit build or benchmark failure, or
/// `None` when it is an infrastructure failure.
///
/// This encodes the contract `docs/DESIGN.md` states under "Backfill": a build or
/// bench failure stops the run by default (or, with `--ignore-errors`, is recorded
/// and skipped with an end-of-run summary), while an infrastructure failure always
/// aborts because continuing cannot produce correct data.
///
/// The reason is embedded mid-line in the human-readable end-of-run summary, so it
/// must be a single line. `ohno` renders a cause as `"\ncaused by: …"` and a
/// captured backtrace as `"\n\nBacktrace:\n…"`, both strictly after the failing
/// error's own message, so taking that message's first line cannot admit either
/// however deep the chain is or whatever `RUST_BACKTRACE` says.
pub(crate) fn bench_failure_reason(error: &AppError) -> Option<String> {
    /// The headline of the `T` in `error`'s chain, if there is one.
    fn headline_of<T: Error + 'static>(error: &AppError) -> Option<String> {
        error.find_source::<T>().map(|failure| headline(failure))
    }

    headline_of::<EngineFailedError>(error)
        .or_else(|| headline_of::<EngineTerminatedError>(error))
        .or_else(|| headline_of::<InvalidCommandError>(error))
        .or_else(|| headline_of::<ParseOutputError>(error))
}

/// The first line of an error's rendered form: its own message, without the cause
/// chain and backtrace `ohno` renders on the lines that follow it.
fn headline(error: &dyn Error) -> String {
    error
        .to_string()
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The benchmark command exited with a non-zero status.
#[derive(ohno::Error)]
#[display("engine {engine:?} failed with exit code {code}")]
pub struct EngineFailedError {
    engine: String,
    code: i32,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for EngineFailedError {}
impl RefUnwindSafe for EngineFailedError {}

impl EngineFailedError {
    /// The benchmark command that failed (`cargo bench`).
    #[must_use]
    pub fn engine(&self) -> &str {
        &self.engine
    }

    /// The process exit code the command reported.
    #[must_use]
    pub fn code(&self) -> i32 {
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
}

/// A `--best-of` run measured a different set of cases or metrics across its
/// repetitions, so the per-metric minimum is not well defined.
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
}

/// A result is already stored for this run's identity (same partition and commit)
/// and the run did not request an overwrite.
#[derive(ohno::Error)]
#[display("a result is already stored for this run at {key}; pass --overwrite to replace it")]
pub struct DuplicateResultError {
    key: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for DuplicateResultError {}
impl RefUnwindSafe for DuplicateResultError {}

impl DuplicateResultError {
    /// The object key that already held a result.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// A backfill precondition failed (a dirty working tree, an unresolvable or
/// out-of-history commit range) or the run stopped after a per-commit failure
/// (without `--ignore-errors`).
///
/// The message carries the explanation and any partial summary.
#[derive(ohno::Error)]
#[display("backfill failed: {message}")]
pub struct BackfillError {
    message: String,

    #[error]
    core: OhnoCore,
}

impl UnwindSafe for BackfillError {}
impl RefUnwindSafe for BackfillError {}

/// An `import` precondition failed (for example a `--commit` override that resolves
/// to no commit in the repository).
#[derive(ohno::Error)]
#[no_constructors]
#[display("import failed: {message}")]
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
    label: String,
    path: PathBuf,

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

        assert!(error.message().contains("callgrind"));
        assert!(error.message().contains("101"));
        assert_eq!(error.engine(), "callgrind");
        assert_eq!(error.code(), 101);
        assert!(error.source().is_none());
    }

    #[test]
    fn engine_termination_reads_differently_from_an_exit_code_failure() {
        // The two cases are reported by separate types precisely because a signalled
        // death has no exit code to name.
        let terminated = EngineTerminatedError::new("callgrind");

        assert!(terminated.message().contains("without an exit code"));
        assert!(terminated.source().is_none());
        assert_ne!(
            terminated.message(),
            EngineFailedError::new("callgrind", 101).message()
        );
    }

    #[test]
    fn invalid_command_names_the_engine_and_the_problem() {
        let error = InvalidCommandError::new("cargo bench", "command is empty");

        assert!(error.message().contains("cargo bench"));
        assert!(error.message().contains("invalid command"));
        assert!(error.message().contains("command is empty"));
    }

    #[test]
    fn parse_failure_describes_the_output_that_could_not_be_read() {
        let error = ParseOutputError::caused_by(
            "target/callgrind/summary.json",
            io::Error::other("missing 'value' field"),
        );

        assert!(error.message().contains("failed to parse benchmark output"));
        assert!(error.message().contains("summary.json"));
        // The parse failure is a source, not text folded into the message.
        assert!(!headline(&error).contains("missing 'value' field"));
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn inconsistent_runs_name_the_engine_and_the_mismatch() {
        let error = InconsistentRunsError::caused_by(
            "criterion",
            io::Error::other("case 'a/b' is missing from run 2"),
        );

        assert!(error.message().contains("--best-of runs"));
        assert!(error.message().contains("criterion"));
        // The mismatch is a source, not text folded into the message.
        assert!(!headline(&error).contains("missing from run 2"));
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn duplicate_result_names_the_key_and_the_escape_hatch() {
        let error = DuplicateResultError::new("v1/folo/objects/callgrind/t/m1/abc/clean.json");

        assert!(error.message().contains("already stored"));
        assert!(error.message().contains("--overwrite"));
        assert_eq!(error.key(), "v1/folo/objects/callgrind/t/m1/abc/clean.json");
        assert!(error.message().contains(error.key()));
    }

    #[test]
    fn backfill_and_import_failures_carry_their_explanations() {
        let backfill = BackfillError::new("the working tree is dirty");
        let import = ImportError::new("--commit resolves to no commit in the repository");

        assert!(backfill.message().contains("backfill failed"));
        assert!(backfill.message().contains("dirty"));
        assert!(import.message().contains("import failed"));
        assert!(import.message().contains("no commit"));
    }

    #[test]
    fn every_io_failure_names_a_distinct_operation() {
        // A bare io::Error says only what the operating system reported and reads the
        // same whichever operation produced it, so each wrapper's own wording is the
        // only thing that tells the failures apart.
        let cause = || io::Error::other("access denied");
        let errors: [AppError; 12] = [
            WorkingDirectoryFailedError::caused_by(cause()).into(),
            BenchCommandFailedError::caused_by("cargo bench", cause()).into(),
            ToolchainProbeFailedError::caused_by(cause()).into(),
            GitProbeFailedError::caused_by(cause()).into(),
            HarvestFailedError::caused_by("criterion", cause()).into(),
            WriteReportFailedError::caused_by("Markdown", "a/report.md", cause()).into(),
            WriteConfigFailedError::caused_by("a/bench-history.toml", cause()).into(),
            ResolveRefFailedError::caused_by("a-ref", cause()).into(),
            FirstParentWalkFailedError::caused_by("b-ref", cause()).into(),
            AddWorktreeFailedError::caused_by("a/tree", "c0ffee", cause()).into(),
            ResetWorktreeFailedError::caused_by("a/tree", "deadbee", cause()).into(),
            RemoveWorktreeFailedError::caused_by("a/tree", cause()).into(),
        ];

        // The first line is the wrapper's own wording; the cause and any backtrace
        // follow it.
        let headlines = errors
            .iter()
            .map(|error| error.message().lines().next().unwrap().to_owned())
            .collect::<Vec<_>>();

        for (error, headline) in errors.iter().zip(&headlines) {
            assert!(error.message().contains("access denied"));
            assert!(error.find_source::<io::Error>().is_some());
            assert_eq!(
                headlines.iter().filter(|other| *other == headline).count(),
                1
            );
        }
    }

    #[test]
    fn io_failures_name_the_subject_they_were_working_on() {
        let cause = || io::Error::other("access denied");

        assert!(
            BenchCommandFailedError::caused_by("cargo bench --bench x", cause())
                .message()
                .contains("cargo bench --bench x")
        );
        assert!(
            HarvestFailedError::caused_by("criterion", cause())
                .message()
                .contains("criterion")
        );
        assert!(
            WriteReportFailedError::caused_by("Markdown", "a/report.md", cause())
                .message()
                .contains("report.md")
        );
        assert!(
            AddWorktreeFailedError::caused_by("a/tree", "c0ffee", cause())
                .message()
                .contains("c0ffee")
        );
        assert!(
            ResetWorktreeFailedError::caused_by("a/tree", "deadbee", cause())
                .message()
                .contains("deadbee")
        );
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
            assert!(bench_failure_reason(error).is_some());
        }
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
            assert!(bench_failure_reason(error).is_none());
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

        let reason = bench_failure_reason(&nested).unwrap();

        // The reason is the bench failure's own wording, not the outer wrapper's.
        assert!(reason.contains("failed to parse benchmark output"));
        assert!(reason.contains("summary.json"));
    }

    #[test]
    fn a_bench_failure_reason_is_one_line_free_of_causes_and_backtraces() {
        // The reason is embedded mid-line in the backfill summary, so nothing the
        // cause chain renders after the failing error's own message may reach it.
        // `ohno` puts both `caused by:` and a captured backtrace on later lines, so
        // a cause whose own rendering carries those shapes stands in for a run with
        // `RUST_BACKTRACE` set without the test having to mutate the environment.
        let backtrace_shaped = io::Error::other(
            "the JSON is malformed\n\nBacktrace:\n   0: cbh_engines::parse_callgrind_summary",
        );
        let error = AppError::from(ParseOutputError::caused_by(
            "a/summary.json",
            backtrace_shaped,
        ));

        // The unabridged rendering does carry the cause; that is what sources are for.
        assert!(error.message().contains("Backtrace:"));

        let reason = bench_failure_reason(&error).unwrap();

        assert_eq!(reason.lines().count(), 1);
        assert!(!reason.contains("Backtrace:"));
        assert!(!reason.contains("caused by:"));
        assert!(!reason.contains("the JSON is malformed"));
        assert!(reason.contains("summary.json"));
    }
}
