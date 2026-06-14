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
    /// Restrict the run to a single named engine, if set.
    pub engine: Option<String>,
    /// Override for the effective timestamp (backfill), if set.
    pub timestamp: Option<Timestamp>,
    /// Override for the recorded target triple, if set.
    pub target_triple: Option<String>,
    /// Harvest and build results without storing them.
    pub no_store: bool,
    /// Arguments forwarded verbatim to each engine command.
    pub passthrough: Vec<String>,
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
    /// Only consider runs on or after this date, if set.
    pub since: Option<String>,
    /// Restrict analysis to a single engine system, if set.
    pub system: Option<String>,
    /// Output format selector, if set.
    pub format: Option<String>,
    /// Exit with failure if a regression is detected.
    pub fail_on_regression: bool,
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
    /// The command is recognized but not yet implemented.
    NotImplemented {
        /// The name of the command that is not yet implemented.
        command: &'static str,
    },
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
    /// No supported engine was available to run.
    NoEngine {
        /// Human-readable explanation of why nothing could run.
        message: String,
    },
    /// An engine command exited with a non-zero status.
    Engine {
        /// The engine whose command failed.
        engine: String,
        /// The process exit code, if one was reported.
        code: Option<i32>,
    },
    /// A harvested benchmark summary could not be parsed.
    Parse {
        /// Human-readable description of the parse failure.
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
            Self::NoEngine { message } => write!(f, "no engine to run: {message}"),
            Self::Engine { engine, code } => match code {
                Some(code) => write!(f, "engine {engine:?} failed with exit code {code}"),
                None => write!(f, "engine {engine:?} terminated without an exit code"),
            },
            Self::Parse { message } => write!(f, "failed to parse benchmark output: {message}"),
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
            Self::NoEngine { .. } | Self::Engine { .. } | Self::Parse { .. } => None,
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
    fn parse_and_no_engine_errors_have_no_source() {
        let parse = RunError::Parse {
            message: "bad".to_owned(),
        };
        let no_engine = RunError::NoEngine {
            message: "none".to_owned(),
        };
        assert!(parse.source().is_none());
        assert!(no_engine.source().is_none());
        assert!(parse.to_string().contains("bad"));
        assert!(no_engine.to_string().contains("none"));
    }

    #[test]
    fn io_error_is_displayed_and_sourced() {
        let error = RunError::from(io::Error::other("broken pipe"));
        assert!(error.to_string().contains("I/O error"));
        assert!(error.source().is_some());
    }
}
