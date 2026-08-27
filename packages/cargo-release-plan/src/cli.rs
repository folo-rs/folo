//! Command-line argument parsing for `cargo-release-plan`, built on `clap`.
//!
//! Parsing accepts the argument vector Cargo passes to a subcommand, including
//! the injected `release-plan` token, and yields either a typed [`RunInput`] or
//! an [`EarlyExit`] carrying the message and exit code to surface.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Error as ClapError, Parser, Subcommand, ValueEnum};

use crate::check::CheckFormat;
use crate::run::RunInput;

/// Default base revision when the caller does not pass `--base`.
///
/// CI should pass an explicit SHA of the merge-base or target-branch tip. A
/// local run compares against the default remote mainline. A stale default can
/// both add and hide differences, so it is not a conservative fallback.
const DEFAULT_BASE: &str = "origin/main";

/// Classifies publishable packages against version anchors and applies plans.
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

impl Cli {
    /// Parses argv into a typed CLI, or an early-exit help/error outcome.
    ///
    /// # Errors
    ///
    /// Returns an [`EarlyExit`] when the arguments request help/usage or fail to
    /// parse.
    pub fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, EarlyExit> {
        let argv: Vec<&str> = command_name.iter().chain(args).copied().collect();
        Self::from_args_os(argv)
    }

    /// Parses OS arguments, stripping the `release-plan` token Cargo injects.
    pub fn from_args_os<I, T>(args: I) -> Result<Self, EarlyExit>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let mut argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
        if argv.get(1).is_some_and(|arg| arg == "release-plan") {
            argv.remove(1);
        }
        Self::try_parse_from(argv).map_err(|error| EarlyExit::from_clap(&error))
    }

    /// Translates the parsed arguments into the [`RunInput`] the core logic consumes.
    ///
    /// Optional arguments are resolved here, so the returned value is fully
    /// determined and carries no further defaults.
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

/// Clap grammar for the subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Write report.json and per-package diffs for unreleased changes.
    Report(ReportArgs),
    /// Fail on unreleased changes or an inconsistent version group.
    Check(CheckArgs),
    /// Apply an approved increment plan to manifests and the lockfile.
    Apply(ApplyArgs),
}

/// Arguments for `report`.
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

/// Arguments for `check`.
#[derive(Debug, Parser)]
struct CheckArgs {
    /// Base revision whose first-parent line supplies anchors.
    #[arg(long, default_value = DEFAULT_BASE)]
    base: String,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// How to render diagnostics.
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

/// Arguments for `apply`.
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

/// Clap value for `--format`; converted to [`CheckFormat`].
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
/// error (failure, printed to stderr).
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
    fn from_clap(error: &ClapError) -> Self {
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::{Cli, EarlyExit};
    use crate::RunInput;

    assert_impl_all!(Cli: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EarlyExit: UnwindSafe, RefUnwindSafe);

    #[test]
    fn from_args_os_strips_cargo_injected_subcommand() {
        let cli = Cli::from_args_os(["cargo-release-plan", "release-plan", "check"]).unwrap();
        match cli.into_input() {
            RunInput::Check { .. } => {}
            other => panic!("expected check, got {other:?}"),
        }
    }
}
