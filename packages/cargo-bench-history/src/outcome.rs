//! The result of executing a [`Command`](crate::Command): the [`RunOutcome`] a
//! successful [`run`](crate::run) returns and the [`RunError`] it fails with.

use std::error::Error;
use std::fmt;
use std::io;

use crate::{ConfigError, StorageError};
/// The outcome of a successful [`run`](crate::run).
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
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
        /// Number of flagged regressions across all analyzed series, for
        /// informational use. It never affects the process exit code: findings
        /// are advisory, so the machine-readable signal lives in the report's
        /// JSON (`notable`), not in the exit status.
        regressions: usize,
    },
}

impl RunOutcome {
    /// Whether the command should be considered successful (exit code zero).
    ///
    /// Every outcome is successful: a finding is never a build-failing condition.
    /// Only an actual [`RunError`](crate::RunError) (a failure to *run*) yields a
    /// non-zero exit code. Downstream automation reads notable findings from the
    /// report JSON rather than from the exit status.
    #[must_use]
    // Every outcome is successful, so this is effectively a constant `true`; a
    // `false` mutant is unkillable because no failing outcome exists to assert
    // against. The constant is the deliberate contract, not a coverage gap.
    #[cfg_attr(test, mutants::skip)]
    pub fn is_success(&self) -> bool {
        match self {
            Self::Completed { .. } | Self::Analyzed { .. } => true,
        }
    }

    /// The text to print to standard output, or `None` when there is nothing to
    /// print.
    ///
    /// `--no-text` suppresses the text report, leaving the message/report empty;
    /// in that case this returns `None` so the caller emits no output at all
    /// rather than a blank line (the requested Markdown/JSON files carry the
    /// result instead).
    #[must_use]
    pub fn stdout_text(&self) -> Option<&str> {
        let text = match self {
            Self::Completed { message } => message,
            Self::Analyzed { report, .. } => report,
        };
        (!text.is_empty()).then_some(text.as_str())
    }
}

/// An error from [`run`](crate::run).
#[doc(hidden)]
#[derive(Debug)]
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
    /// A blessing precondition failed (a dirty working tree, a commit not on the
    /// base branch, no stored result at the current commit, or no prefixes given).
    Bless {
        /// Human-readable description of the precondition failure.
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
            Self::Bless { message } => write!(f, "blessing failed: {message}"),
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
            | Self::Backfill { .. }
            | Self::Bless { .. } => None,
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

/// Finishes a mutating command by combining its `command` outcome with the
/// result of the post-command cache-invalidation `flush`, giving a failed flush
/// precedence over the command's own outcome.
///
/// The flush is armed only when a delete or overwrite reached the shared cloud
/// backend during this command, so it is `Ok` in the common case and this simply
/// returns `command` unchanged. When it *does* fail, a mutation already reached
/// the cloud but its invalidation marker did not — a cross-machine
/// cache-correctness hazard that is invisible at the failing command and would
/// leave *other* machines' read-through caches stale. Surfacing it first (rather
/// than after a `command?` that may short-circuit) guarantees the stale-cache
/// failure is never silently dropped, even when the command itself also failed.
pub(crate) fn finish_with_flush(
    command: Result<RunOutcome, RunError>,
    flush: Result<(), StorageError>,
) -> Result<RunOutcome, RunError> {
    flush?;
    command
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
            key: "v1/folo/callgrind/t/synthetic/abc/clean.json".to_owned(),
        };
        assert!(error.to_string().contains("already stored"), "{error}");
        assert!(error.to_string().contains("--overwrite"), "{error}");
        assert!(
            error
                .to_string()
                .contains("v1/folo/callgrind/t/synthetic/abc/clean.json"),
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
    fn analyzed_outcome_is_always_successful() {
        // Findings are advisory and never fail a build: an Analyzed outcome is
        // successful regardless of how many regressions it flagged.
        assert!(
            RunOutcome::Analyzed {
                report: "r".to_owned(),
                regressions: 3,
            }
            .is_success()
        );
        assert!(
            RunOutcome::Analyzed {
                report: "r".to_owned(),
                regressions: 0,
            }
            .is_success()
        );
    }

    #[test]
    fn stdout_text_returns_nonempty_message_and_report() {
        assert_eq!(
            RunOutcome::Completed {
                message: "done".to_owned(),
            }
            .stdout_text(),
            Some("done")
        );
        assert_eq!(
            RunOutcome::Analyzed {
                report: "report body".to_owned(),
                regressions: 2,
            }
            .stdout_text(),
            Some("report body")
        );
    }

    #[test]
    fn stdout_text_is_suppressed_when_the_report_is_empty() {
        // `--no-text` leaves the message/report empty; the caller must print
        // nothing rather than a blank line.
        assert_eq!(
            RunOutcome::Completed {
                message: String::new(),
            }
            .stdout_text(),
            None
        );
        assert_eq!(
            RunOutcome::Analyzed {
                report: String::new(),
                regressions: 0,
            }
            .stdout_text(),
            None
        );
    }

    // A distinct command failure, so a test can tell whether the command error or
    // the flush error came back out of `finish_with_flush`.
    fn command_failure() -> RunError {
        RunError::Bless {
            message: "a bless precondition failed".to_owned(),
        }
    }

    // A distinct flush failure (a `StorageError`, surfaced as `RunError::Storage`),
    // so it is unambiguously different from `command_failure`.
    fn flush_failure() -> StorageError {
        StorageError::NotFound {
            key: "cache-epoch".to_owned(),
        }
    }

    #[test]
    fn finish_with_flush_returns_the_outcome_when_both_succeed() {
        let outcome = finish_with_flush(
            Ok(RunOutcome::Completed {
                message: "done".to_owned(),
            }),
            Ok(()),
        )
        .expect("both succeeded");
        assert_eq!(
            outcome,
            RunOutcome::Completed {
                message: "done".to_owned(),
            }
        );
    }

    #[test]
    fn finish_with_flush_surfaces_a_flush_failure_after_a_successful_command() {
        // The command stored data that reached the cloud, but the invalidation
        // marker write failed — the flush error must not be swallowed just because
        // the command itself succeeded.
        let error = finish_with_flush(
            Ok(RunOutcome::Completed {
                message: "done".to_owned(),
            }),
            Err(flush_failure()),
        )
        .expect_err("the flush failed");
        assert!(
            matches!(error, RunError::Storage(_)),
            "expected the flush error, got {error:?}"
        );
    }

    #[test]
    fn finish_with_flush_returns_the_command_error_when_the_flush_succeeds() {
        let error =
            finish_with_flush(Err(command_failure()), Ok(())).expect_err("the command failed");
        assert!(
            matches!(error, RunError::Bless { .. }),
            "expected the command error, got {error:?}"
        );
    }

    #[test]
    fn finish_with_flush_prefers_the_flush_failure_over_a_failed_command() {
        // The regression guard: a delete/overwrite reached the cloud (arming the
        // marker) *and* the command later failed. The flush failure — which leaves
        // other machines' caches stale — must take precedence rather than being
        // silently dropped by the command's own early return.
        let error = finish_with_flush(Err(command_failure()), Err(flush_failure()))
            .expect_err("both failed");
        assert!(
            matches!(error, RunError::Storage(_)),
            "expected the flush error to win, got {error:?}"
        );
    }
}
