//! Command-line argument parsing for `cargo-release-plan`, built on `clap`.
//!
//! The parser lives in the library rather than the binary so its behavior —
//! argument defaults, help text, and parse errors — can be exercised directly by
//! integration tests without spawning a subprocess.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::types::DEFAULT_BASE;
use crate::{CheckFormat, RunInput};

/// A Cargo subcommand that classifies publishable packages against version anchors
/// and applies increment plans.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-release-plan",
    about = "Classify publishable packages against version anchors and apply increment plans.",
    disable_version_flag = true
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write report.json and per-package diffs for unreleased changes.
    Report(ReportArgs),
    /// Fail on unreleased changes or an inconsistent version group.
    Check(CheckArgs),
    /// Apply an approved increment plan to manifests and the lockfile.
    Apply(ApplyArgs),
}

#[derive(Debug, Parser)]
struct ReportArgs {
    /// Directory that receives `report.json` and `diffs/`.
    #[arg(long)]
    out_dir: PathBuf,

    /// Base revision whose first-parent line supplies anchors.
    #[arg(long, default_value = DEFAULT_BASE)]
    base: String,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Print explanatory notes for each classification decision.
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Parser)]
struct CheckArgs {
    /// Base revision whose first-parent line supplies anchors.
    #[arg(long, default_value = DEFAULT_BASE)]
    base: String,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// How to render offences.
    #[arg(long, value_enum, default_value_t = CliCheckFormat::Text)]
    format: CliCheckFormat,

    /// Warn when released-content rules diverge from `cargo package --list`.
    ///
    /// Non-gating: a mismatch is printed and the check verdict is unchanged.
    #[arg(long)]
    verify_packaging: bool,

    /// Print explanatory notes for each classification decision.
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Parser)]
struct ApplyArgs {
    /// Path to the approved plan JSON file.
    #[arg(long)]
    plan: PathBuf,

    /// Compute edits without writing files or refreshing the lockfile.
    #[arg(long)]
    dry_run: bool,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Print explanatory notes for each edit decision.
    #[arg(long)]
    verbose: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CliCheckFormat {
    Text,
    Github,
}

impl From<CliCheckFormat> for CheckFormat {
    fn from(value: CliCheckFormat) -> Self {
        match value {
            CliCheckFormat::Text => Self::Text,
            CliCheckFormat::Github => Self::Github,
        }
    }
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

    /// Translates the parsed arguments into the [`RunInput`] the core logic consumes.
    #[must_use]
    pub fn into_input(self) -> RunInput {
        match self.command {
            Command::Report(args) => RunInput::Report {
                out_dir: args.out_dir,
                base: args.base,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
            Command::Check(args) => RunInput::Check {
                base: args.base,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                format: args.format.into(),
                verify_packaging: args.verify_packaging,
                verbose: args.verbose,
            },
            Command::Apply(args) => RunInput::Apply {
                plan: args.plan,
                dry_run: args.dry_run,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::{Cli, EarlyExit};

    assert_impl_all!(Cli: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EarlyExit: UnwindSafe, RefUnwindSafe);
}
