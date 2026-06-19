//! The typed command model the binary and tests operate on: the parsed
//! [`Command`] and its per-subcommand options, plus the [`RunOutcome`] and
//! [`RunError`] the [`run`](crate::run) entry point returns.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use jiff::Timestamp;

use crate::{ConfigError, StorageError};

/// A fully parsed command ready to execute.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub enum Command {
    /// Run the configured benchmark engines and store the results.
    Run(RunOptions),
    /// Generate a starter configuration file.
    Install(InstallOptions),
    /// Analyze stored history for notable patterns.
    Analyze(AnalyzeOptions),
    /// List the data set a matching `analyze` pass would include.
    List(ListOptions),
    /// Replay `run` across a range of historical commits.
    Backfill(BackfillOptions),
}

/// Options for the `run` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub struct RunOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Restrict the run to these packages (`--package`/`-p`); empty means the
    /// whole workspace.
    pub packages: Vec<String>,
    /// Restrict the run to these benchmark targets (`--bench`); empty means all.
    pub benches: Vec<String>,
    /// Override for the effective timestamp (backfill), if set.
    pub timestamp: Option<Timestamp>,
    /// Override for the recorded target triple, if set.
    pub target_triple: Option<String>,
    /// Override for the machine fingerprint (hardware-dependent engines), if set.
    pub machine_key: Option<String>,
    /// Harvest and build results without storing them.
    pub no_store: bool,
    /// Replace an already-stored result for this run's identity instead of
    /// refusing the run as a duplicate.
    pub overwrite: bool,
    /// Arguments forwarded verbatim to the benchmark command after the scope flags.
    pub passthrough: Vec<String>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `install` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub struct InstallOptions {
    /// Path to the configuration file to generate, if overridden.
    pub config_path: Option<PathBuf>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `analyze` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub struct AnalyzeOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Target ref whose history is analyzed; defaults to `HEAD`.
    pub branch: Option<String>,
    /// Base ref the target's history is split at; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Exclude dirty (uncommitted-tree) snapshots from the target side.
    pub no_dirty: bool,
    /// Only consider runs on or after this date, if set.
    pub since: Option<String>,
    /// Restrict analysis to a single engine (criterion or callgrind), if set.
    pub engine: Option<String>,
    /// Restrict analysis to a single full target triple, if set. Mutually
    /// exclusive with `os` / `architecture` (the triple already fixes both).
    pub target_triple: Option<String>,
    /// Restrict analysis to a single operating-system facet, if set.
    pub os: Option<String>,
    /// Restrict analysis to a single CPU-architecture facet, if set.
    pub architecture: Option<String>,
    /// Restrict analysis to a single machine partition, if set.
    pub machine_key: Option<String>,
    /// Restrict analysis to a single metric name, if set.
    pub metric: Option<String>,
    /// Output format selector, if set.
    pub format: Option<String>,
    /// Exit with failure if a regression is detected.
    pub fail_on_regression: bool,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `list` command.
///
/// The data-set-selection options mirror [`AnalyzeOptions`] exactly so a `list`
/// invocation previews the data set the same `analyze` invocation would consume.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub struct ListOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Target ref whose history is listed; defaults to `HEAD`.
    pub branch: Option<String>,
    /// Base ref the target's history is split at; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Exclude dirty (uncommitted-tree) snapshots from the target side.
    pub no_dirty: bool,
    /// Only consider runs on or after this date, if set.
    pub since: Option<String>,
    /// Restrict the listing to a single engine (criterion or callgrind), if set.
    pub engine: Option<String>,
    /// Restrict the listing to a single full target triple, if set. Mutually
    /// exclusive with `os` / `architecture` (the triple already fixes both).
    pub target_triple: Option<String>,
    /// Restrict the listing to a single operating-system facet, if set.
    pub os: Option<String>,
    /// Restrict the listing to a single CPU-architecture facet, if set.
    pub architecture: Option<String>,
    /// Restrict the listing to a single machine partition, if set.
    pub machine_key: Option<String>,
    /// Restrict the listing to a single metric name, if set.
    pub metric: Option<String>,
    /// Output format selector, if set.
    pub format: Option<String>,
    /// List the discriminant sets present in storage instead of the data set that
    /// would enter the analysis. Does not require a repository.
    pub discriminants: bool,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `backfill` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub struct BackfillOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Oldest commit of the range to backfill (inclusive).
    pub from: String,
    /// Newest commit of the range to backfill (inclusive).
    pub to: String,
    /// Restrict the runs to these packages (`--package`/`-p`); empty means the
    /// whole workspace.
    pub packages: Vec<String>,
    /// Restrict the runs to these benchmark targets (`--bench`); empty means all.
    pub benches: Vec<String>,
    /// Override for the recorded target triple, if set.
    pub target_triple: Option<String>,
    /// Override for the machine fingerprint (hardware-dependent engines), if set.
    pub machine_key: Option<String>,
    /// Replace already-stored results for the backfilled commits instead of
    /// skipping them as duplicates.
    pub overwrite: bool,
    /// Continue past commits whose build or benchmark fails instead of stopping.
    pub ignore_errors: bool,
    /// Arguments forwarded verbatim to the benchmark command after the scope flags.
    pub passthrough: Vec<String>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// The outcome of a successful [`run`](crate::run).
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub enum RunOutcome {
    /// The command completed; `message` is a human-readable summary.
    Completed {
        /// Human-readable summary of what happened.
        message: String,
    },
    /// The `analyze` command produced a findings report.
    Analyzed {
        /// The rendered findings report for the requested output format.
        report: String,
        /// Number of flagged regressions across all analyzed series.
        regressions: usize,
        /// Whether `--fail-on-regression` was set; with a non-zero
        /// `regressions` count it makes the command exit unsuccessfully.
        fail_on_regression: bool,
    },
}

impl RunOutcome {
    /// Whether the command should be considered successful (exit code zero).
    ///
    /// Every outcome is successful except an [`Analyzed`](Self::Analyzed) report
    /// that flagged at least one regression while `--fail-on-regression` was set.
    #[must_use]
    pub fn is_success(&self) -> bool {
        match self {
            Self::Completed { .. } => true,
            Self::Analyzed {
                regressions,
                fail_on_regression,
                ..
            } => !(*fail_on_regression && *regressions > 0),
        }
    }
}

/// An error from [`run`](crate::run).
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "constructed and matched by the in-crate binary and integration tests"
)]
pub enum RunError {
    /// Loading or parsing configuration failed.
    Config(ConfigError),
    /// A storage operation failed.
    Storage(StorageError),
    /// The benchmark command exited with a non-zero status.
    Engine {
        /// The benchmark command that failed (`cargo bench`).
        engine: String,
        /// The process exit code, if one was reported.
        code: Option<i32>,
    },
    /// The benchmark command could not be assembled into an argv.
    Command {
        /// The benchmark command label.
        engine: String,
        /// Human-readable description of the failure.
        message: String,
    },
    /// A harvested benchmark summary could not be parsed.
    Parse {
        /// Human-readable description of the parse failure.
        message: String,
    },
    /// A result is already stored for this run's identity (same partition and
    /// commit) and the run did not request an overwrite.
    Duplicate {
        /// The object key that already held a result.
        key: String,
    },
    /// Analyzing stored history failed (bad filter, malformed stored object).
    Analyze {
        /// Human-readable description of the analysis failure.
        message: String,
    },
    /// A backfill precondition failed (a dirty working tree, an unresolvable or
    /// out-of-history commit range) or the run stopped after a per-commit
    /// failure (without `--ignore-errors`). The message carries the explanation
    /// and any partial summary.
    Backfill {
        /// Human-readable description, including any partial backfill summary.
        message: String,
    },
    /// An underlying I/O operation (process, probe, or harvest) failed.
    Io(io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "configuration error: {error}"),
            Self::Storage(error) => write!(f, "storage error: {error}"),
            Self::Engine { engine, code } => match code {
                Some(code) => write!(f, "engine {engine:?} failed with exit code {code}"),
                None => write!(f, "engine {engine:?} terminated without an exit code"),
            },
            Self::Command { engine, message } => {
                write!(f, "engine {engine:?} has an invalid command: {message}")
            }
            Self::Parse { message } => write!(f, "failed to parse benchmark output: {message}"),
            Self::Duplicate { key } => write!(
                f,
                "a result is already stored for this run at {key}; pass --overwrite to replace it"
            ),
            Self::Analyze { message } => write!(f, "failed to analyze history: {message}"),
            Self::Backfill { message } => write!(f, "backfill failed: {message}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Engine { .. }
            | Self::Command { .. }
            | Self::Parse { .. }
            | Self::Duplicate { .. }
            | Self::Analyze { .. }
            | Self::Backfill { .. } => None,
        }
    }
}

impl From<ConfigError> for RunError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<StorageError> for RunError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<io::Error> for RunError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn config_error_is_displayed_and_sourced() {
        let error = RunError::from(ConfigError::Parse("bad".to_owned()));
        assert!(error.to_string().contains("configuration error"));
        assert!(error.source().is_some());
    }

    #[test]
    fn storage_error_is_displayed_and_sourced() {
        let error = RunError::from(StorageError::NotFound {
            key: "k".to_owned(),
        });
        assert!(error.to_string().contains("storage error"));
        assert!(error.source().is_some());
    }

    #[test]
    fn engine_error_renders_exit_code() {
        let error = RunError::Engine {
            engine: "callgrind".to_owned(),
            code: Some(101),
        };
        assert!(error.to_string().contains("101"));
        assert!(error.source().is_none());
    }

    #[test]
    fn engine_error_without_code_renders_message() {
        let error = RunError::Engine {
            engine: "callgrind".to_owned(),
            code: None,
        };
        assert!(
            error.to_string().contains("without an exit code"),
            "{error}"
        );
        assert!(error.source().is_none());
    }

    #[test]
    fn parse_and_command_errors_have_no_source() {
        let parse = RunError::Parse {
            message: "bad".to_owned(),
        };
        let command = RunError::Command {
            engine: "cargo bench".to_owned(),
            message: "empty".to_owned(),
        };
        assert!(parse.source().is_none());
        assert!(command.source().is_none());
        assert!(parse.to_string().contains("bad"));
        assert!(command.to_string().contains("empty"));
    }

    #[test]
    fn duplicate_error_mentions_overwrite_and_has_no_source() {
        let error = RunError::Duplicate {
            key: "v2/folo/callgrind/t/synthetic/abc/clean.json".to_owned(),
        };
        assert!(error.to_string().contains("already stored"), "{error}");
        assert!(error.to_string().contains("--overwrite"), "{error}");
        assert!(
            error
                .to_string()
                .contains("v2/folo/callgrind/t/synthetic/abc/clean.json"),
            "{error}"
        );
        assert!(error.source().is_none());
    }

    #[test]
    fn analyze_error_is_displayed_and_has_no_source() {
        let error = RunError::Analyze {
            message: "unknown report format".to_owned(),
        };
        assert!(
            error.to_string().contains("failed to analyze history"),
            "{error}"
        );
        assert!(
            error.to_string().contains("unknown report format"),
            "{error}"
        );
        assert!(error.source().is_none());
    }

    #[test]
    fn backfill_error_is_displayed_and_has_no_source() {
        let error = RunError::Backfill {
            message: "refusing to backfill: the working tree is dirty".to_owned(),
        };
        assert!(error.to_string().contains("backfill failed"), "{error}");
        assert!(error.to_string().contains("dirty"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn command_error_is_displayed_and_has_no_source() {
        let error = RunError::Command {
            engine: "callgrind".to_owned(),
            message: "command is empty".to_owned(),
        };
        assert!(error.to_string().contains("invalid command"), "{error}");
        assert!(error.to_string().contains("command is empty"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn io_error_is_displayed_and_sourced() {
        let error = RunError::from(io::Error::other("broken pipe"));
        assert!(error.to_string().contains("I/O error"));
        assert!(error.source().is_some());
    }

    #[test]
    fn completed_outcome_is_successful() {
        assert!(
            RunOutcome::Completed {
                message: "done".to_owned(),
            }
            .is_success()
        );
    }

    #[test]
    fn analyzed_outcome_fails_only_when_gated_and_regressed() {
        let gated_with_regression = RunOutcome::Analyzed {
            report: "r".to_owned(),
            regressions: 1,
            fail_on_regression: true,
        };
        assert!(!gated_with_regression.is_success());

        // A regression without the gate, or the gate without a regression, both
        // still succeed.
        assert!(
            RunOutcome::Analyzed {
                report: "r".to_owned(),
                regressions: 3,
                fail_on_regression: false,
            }
            .is_success()
        );
        assert!(
            RunOutcome::Analyzed {
                report: "r".to_owned(),
                regressions: 0,
                fail_on_regression: true,
            }
            .is_success()
        );
    }
}
