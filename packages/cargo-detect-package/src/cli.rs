//! Command-line argument parsing for `cargo-detect-package`, built on `clap`.
//!
//! The parser lives in the library rather than the binary so its behavior —
//! argument defaults, help text, and parse errors — can be exercised directly by
//! integration tests without spawning a subprocess.

use std::path::PathBuf;

use clap::Parser;

use crate::{OutsidePackageAction, RunInput};

/// A Cargo tool to detect the package that a file belongs to, passing the package name
/// to a subcommand.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-detect-package",
    about = "Detect the package a file belongs to and pass its name to a subcommand.",
    disable_version_flag = true
)]
pub struct Cli {
    /// Path to the file to detect the package for.
    #[arg(long)]
    path: PathBuf,

    /// Pass the detected package as an environment variable of this name instead of as a
    /// cargo argument.
    #[arg(long)]
    via_env: Option<String>,

    /// Action to take when the path is not in any package: `workspace`, `ignore`, or
    /// `error`.
    #[arg(long)]
    outside_package: Option<OutsidePackageAction>,

    /// The subcommand to execute, with all of its own arguments.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    subcommand: Vec<String>,
}

/// A parse outcome that should terminate the program before execution.
///
/// This is either a help/usage request (success, printed to stdout) or a parse
/// error (failure, printed to stderr), mirroring the shape the binary entry point
/// consumes.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "handoff struct read directly by the in-crate binary and integration tests"
)]
pub struct EarlyExit {
    /// The rendered message (help text or error) to print.
    pub output: String,
    /// `Ok` for a help/usage request (exit success), `Err` for a parse error.
    pub status: Result<(), ()>,
}

impl EarlyExit {
    /// Classifies a `clap` parse error into the success/failure early-exit shape.
    fn from_clap(error: &clap::Error) -> Self {
        use clap::error::ErrorKind;
        let success = matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        Self {
            output: error.to_string(),
            status: if success { Ok(()) } else { Err(()) },
        }
    }
}

impl Cli {
    /// Parses an argument vector (program name followed by its arguments) into the
    /// typed CLI, returning an [`EarlyExit`] for a help request or a parse error.
    ///
    /// # Errors
    ///
    /// Returns an [`EarlyExit`] when the arguments request help/usage or fail to
    /// parse.
    pub fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, EarlyExit> {
        let argv: Vec<&str> = command_name.iter().chain(args).copied().collect();
        Self::try_parse_from(argv).map_err(|error| EarlyExit::from_clap(&error))
    }

    /// Translates the parsed arguments into the [`RunInput`] the core logic consumes,
    /// defaulting the outside-package action to [`OutsidePackageAction::Workspace`].
    #[must_use]
    pub fn into_input(self) -> RunInput {
        RunInput {
            path: self.path,
            via_env: self.via_env,
            outside_package: self
                .outside_package
                .unwrap_or(OutsidePackageAction::Workspace),
            subcommand: self.subcommand,
        }
    }
}
