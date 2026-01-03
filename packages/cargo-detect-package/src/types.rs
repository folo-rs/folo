// Public API types for cargo-detect-package.
//
// These types are used by main.rs and exposed via the crate's public API.

use std::fmt;
use std::path::PathBuf;

/// Action to take when a path is not within any package.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OutsidePackageAction {
    /// Use the entire workspace.
    Workspace,
    /// Ignore and do not run the subcommand, exit with success.
    Ignore,
    /// Error and do not run the subcommand, exit with failure.
    Error,
}

// Mutations to match arms cause integration test timeouts due to cargo subprocess hangs.
#[cfg_attr(test, mutants::skip)]
impl std::str::FromStr for OutsidePackageAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "workspace" => Ok(Self::Workspace),
            "ignore" => Ok(Self::Ignore),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "Invalid outside-package action: '{s}'. Valid options are: workspace, ignore, error"
            )),
        }
    }
}

/// Input parameters for the `run` function.
///
/// This is the parsed and validated input that the core logic operates on.
#[doc(hidden)]
#[derive(Debug)]
#[allow(
    clippy::exhaustive_structs,
    reason = "This is a hidden struct for internal/test use only"
)]
pub struct RunInput {
    /// Path to the file to detect package for.
    pub path: PathBuf,
    /// Pass the detected package as an environment variable instead of as a cargo argument.
    pub via_env: Option<String>,
    /// Action to take when path is not in any package.
    pub outside_package: OutsidePackageAction,
    /// The subcommand to execute.
    pub subcommand: Vec<String>,
}

/// The outcome of a successful run.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::exhaustive_enums,
    reason = "This is a hidden enum for internal/test use only"
)]
pub enum RunOutcome {
    /// A package was detected and the subcommand executed successfully.
    PackageDetected {
        /// The name of the detected package.
        package_name: String,
        /// Whether the subcommand succeeded.
        subcommand_succeeded: bool,
    },
    /// The path was not in any package, workspace scope was used.
    WorkspaceScope {
        /// Whether the subcommand succeeded.
        subcommand_succeeded: bool,
    },
    /// The path was not in any package and was ignored (no subcommand executed).
    Ignored,
}

/// Errors that can occur during a run.
#[doc(hidden)]
#[derive(Debug)]
#[allow(
    clippy::exhaustive_enums,
    reason = "This is a hidden enum for internal/test use only"
)]
pub enum RunError {
    /// Failed to validate workspace context.
    WorkspaceValidation(String),
    /// Failed to detect package.
    PackageDetection(String),
    /// Path is not in any package and --outside-package=error was specified.
    OutsidePackage,
    /// Failed to execute the subcommand.
    CommandExecution(std::io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceValidation(msg) => write!(f, "{msg}"),
            Self::PackageDetection(msg) => write!(f, "Error detecting package: {msg}"),
            Self::OutsidePackage => write!(f, "Path is not in any package"),
            Self::CommandExecution(e) => write!(f, "Error executing command: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn outside_package_action_parsing() {
        assert_eq!(
            "workspace".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Workspace
        );
        assert_eq!(
            "Workspace".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Workspace
        );
        assert_eq!(
            "WORKSPACE".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Workspace
        );

        assert_eq!(
            "ignore".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Ignore
        );
        assert_eq!(
            "Ignore".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Ignore
        );

        assert_eq!(
            "error".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Error
        );
        assert_eq!(
            "Error".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Error
        );

        let result = "invalid".parse::<OutsidePackageAction>();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Invalid outside-package action")
        );
    }
}
