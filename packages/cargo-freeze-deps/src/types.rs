// Public API types for cargo-freeze-deps.
//
// These types are used by main.rs and exposed via the crate's public API so that integration
// tests can exercise the core logic without spawning a subprocess.

use std::path::PathBuf;
use std::{error, fmt, io};

/// Input parameters for the [`run`](crate::run) function.
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "Hidden struct for internal/test use only"
)]
pub struct RunInput {
    /// Path to the Cargo.toml file to read.
    pub path: PathBuf,
    /// Optional path to write the rewritten Cargo.toml to.
    ///
    /// When `None`, the file at `path` is rewritten in place.
    pub output: Option<PathBuf>,
}

/// The successful outcome of a run.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "Hidden struct for internal/test use only"
)]
pub struct RunOutcome {
    /// Number of dependency version requirements that were rewritten.
    pub frozen_count: usize,
    /// Number of dependency version requirements that were left as-is because they
    /// did not have any freezable component (e.g. `<1.2.3`, bare `*`).
    pub skipped_count: usize,
}

/// Errors that can occur during a run.
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hidden enum for internal/test use only"
)]
pub enum RunError {
    /// Failed to read or write the Cargo.toml file.
    Io(io::Error),
    /// The Cargo.toml file could not be parsed as TOML.
    Parse(String),
    /// A dependency's version field had an unexpected non-string type.
    UnexpectedVersionType {
        /// Name of the dependency whose version field is malformed.
        dep: String,
        /// The actual TOML type encountered, for diagnostics.
        actual_type: String,
    },
    /// A dependency's version requirement could not be parsed as a semver requirement.
    InvalidVersion {
        /// Name of the dependency whose version requirement is invalid.
        dep: String,
        /// The literal text of the requirement.
        version: String,
        /// The semver parse error.
        source: semver::Error,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Parse(msg) => write!(f, "Failed to parse Cargo.toml: {msg}"),
            Self::UnexpectedVersionType { dep, actual_type } => write!(
                f,
                "Dependency '{dep}' has a non-string version field of type {actual_type}"
            ),
            Self::InvalidVersion {
                dep,
                version,
                source,
            } => write!(
                f,
                "Dependency '{dep}' has invalid version requirement '{version}': {source}"
            ),
        }
    }
}

impl error::Error for RunError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::InvalidVersion { source, .. } => Some(source),
            Self::Parse(_) | Self::UnexpectedVersionType { .. } => None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn run_error_display_io() {
        let err = RunError::Io(io::Error::new(io::ErrorKind::NotFound, "missing"));
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn run_error_display_parse() {
        let err = RunError::Parse("bad toml".to_string());
        assert!(!format!("{err}").is_empty());
    }

    #[test]
    fn run_error_display_unexpected_version_type() {
        let err = RunError::UnexpectedVersionType {
            dep: "serde".to_string(),
            actual_type: "integer".to_string(),
        };
        let display = format!("{err}");
        assert!(display.contains("serde"));
        assert!(display.contains("integer"));
    }

    #[test]
    fn run_error_display_invalid_version() {
        let req_err = "not-a-version".parse::<semver::VersionReq>().unwrap_err();
        let err = RunError::InvalidVersion {
            dep: "serde".to_string(),
            version: "not-a-version".to_string(),
            source: req_err,
        };
        let display = format!("{err}");
        assert!(display.contains("serde"));
        assert!(display.contains("not-a-version"));
    }

    #[test]
    fn run_error_source_chain() {
        let err = RunError::Io(io::Error::new(io::ErrorKind::NotFound, "missing"));
        assert!(error::Error::source(&err).is_some());

        let err = RunError::Parse("bad toml".to_string());
        assert!(error::Error::source(&err).is_none());

        let err = RunError::UnexpectedVersionType {
            dep: "x".to_string(),
            actual_type: "y".to_string(),
        };
        assert!(error::Error::source(&err).is_none());

        let req_err = "not-a-version".parse::<semver::VersionReq>().unwrap_err();
        let err = RunError::InvalidVersion {
            dep: "x".to_string(),
            version: "x".to_string(),
            source: req_err,
        };
        assert!(error::Error::source(&err).is_some());
    }
}
