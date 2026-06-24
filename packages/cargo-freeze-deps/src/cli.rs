//! Command-line argument parsing for `cargo-freeze-deps`, built on `clap`.
//!
//! The parser lives in the library rather than the binary so its behavior —
//! argument defaults, help text, and parse errors — can be exercised directly by
//! integration tests without spawning a subprocess.

use std::path::PathBuf;

use clap::Parser;

use crate::RunInput;

/// A Cargo subcommand that freezes all floating dependency versions in a `Cargo.toml`
/// file to their literal values.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-freeze-deps",
    about = "Freeze every floating dependency version in a Cargo.toml file to its literal value.",
    disable_version_flag = true
)]
pub struct Cli {
    /// Path to the `Cargo.toml` file to freeze. Defaults to `./Cargo.toml` in the
    /// current working directory.
    #[arg(short, long)]
    path: Option<PathBuf>,

    /// Path to write the rewritten `Cargo.toml` to. Defaults to the input path
    /// (rewriting in place).
    #[arg(short, long)]
    output: Option<PathBuf>,
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
    /// applying the default input path (`Cargo.toml`) when none was given.
    #[must_use]
    pub fn into_input(self) -> RunInput {
        RunInput {
            path: self.path.unwrap_or_else(|| PathBuf::from("Cargo.toml")),
            output: self.output,
        }
    }
}
