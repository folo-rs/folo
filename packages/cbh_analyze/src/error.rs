//! The error the `analyze`-family commands fail with: [`AnalyzeError`].

use std::error::Error;
use std::{fmt, io};

use cbh_config::ConfigError;
use cbh_storage::StorageError;

/// An error from an `analyze`-family command (`analyze`, `list`, `prune`,
/// `examine`, `bless`, `unbless`).
///
/// These are the only failure modes this crate produces; the binary's aggregate
/// `RunError` carries the remaining run-level variants (engine, backfill, …) and
/// folds an `AnalyzeError` into itself when dispatching one of these commands. The
/// `Config`, `Storage`, and `Io` variants mirror the binary's wording exactly, so
/// the message a user sees is unchanged by where the command now lives.
#[derive(Debug)]
pub enum AnalyzeError {
    /// Loading or parsing configuration failed.
    Config(ConfigError),
    /// A storage operation failed.
    Storage(StorageError),
    /// Analyzing stored history failed (bad filter, malformed stored object, or no
    /// output selected).
    Analyze {
        /// Human-readable description of the analysis failure.
        message: String,
    },
    /// A blessing precondition failed (a dirty working tree, a commit not on the
    /// base branch, no stored result at the current commit, or no prefixes given).
    Bless {
        /// Human-readable description of the precondition failure.
        message: String,
    },
    /// An underlying I/O operation failed.
    Io(io::Error),
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "configuration error: {error}"),
            Self::Storage(error) => write!(f, "storage error: {error}"),
            Self::Analyze { message } => write!(f, "failed to analyze history: {message}"),
            Self::Bless { message } => write!(f, "blessing failed: {message}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
        }
    }
}

impl Error for AnalyzeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Analyze { .. } | Self::Bless { .. } => None,
        }
    }
}

impl From<ConfigError> for AnalyzeError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<StorageError> for AnalyzeError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<io::Error> for AnalyzeError {
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
        let error = AnalyzeError::from(ConfigError::new("bad"));
        assert!(error.to_string().contains("configuration error"), "{error}");
        assert!(error.source().is_some());
    }

    #[test]
    fn storage_error_is_displayed_and_sourced() {
        let error = AnalyzeError::from(StorageError::NotFound {
            key: "k".to_owned(),
        });
        assert!(error.to_string().contains("storage error"), "{error}");
        assert!(error.source().is_some());
    }

    #[test]
    fn analyze_error_is_displayed_and_has_no_source() {
        let error = AnalyzeError::Analyze {
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
    fn bless_error_is_displayed_and_has_no_source() {
        let error = AnalyzeError::Bless {
            message: "a bless precondition failed".to_owned(),
        };
        assert!(error.to_string().contains("blessing failed"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn io_error_is_displayed_and_sourced() {
        let error = AnalyzeError::from(io::Error::other("broken pipe"));
        assert!(error.to_string().contains("I/O error"), "{error}");
        assert!(error.source().is_some());
    }
}
