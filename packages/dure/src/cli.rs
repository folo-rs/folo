//! Command-line argument parsing for `dure`, built on `clap`.
//!
//! The parser lives in the library so parse behavior can be tested without
//! spawning a subprocess.

use std::num::NonZero;
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

use crate::constants::SUPERVISOR_COMMAND;
use crate::session_id::SessionId;
use crate::types::{Command, RunInput};

/// Clap-facing parser for the `dure` binary.
///
/// Translates argv into [`RunInput`] so [`crate::run`] does not depend on clap.
/// Version is omitted because this crate is unpublished; `--version` would
/// report a workspace placeholder rather than a released artifact.
#[derive(Debug, Parser)]
#[command(
    name = "dure",
    about = "Detachable Windows console sessions that outlive the terminal.",
    disable_version_flag = true
)]
pub struct Cli {
    /// Explain auto-detect decisions on stderr.
    #[arg(long, global = true)]
    verbose: bool,

    /// Override the session store root.
    ///
    /// Hidden; integration tests use this so they never touch `LocalAppData`.
    #[arg(long, global = true, hide = true)]
    store_root: Option<PathBuf>,

    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Start a new session and attach immediately.
    Run {
        /// Command to execute directly, not through a shell.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Attach to a live session.
    Resume {
        /// Session id to attach to, skipping auto-detect.
        #[arg(long)]
        id: Option<NonZero<u32>>,
    },
    /// Print live sessions.
    List,
    /// Abruptly terminate the supervisor for a session.
    Kill {
        /// Session id to kill. Required; kill does not auto-detect.
        #[arg(long)]
        id: NonZero<u32>,
    },
    /// Hidden supervisor process started by `dure run`.
    #[command(name = SUPERVISOR_COMMAND, hide = true)]
    Supervisor {
        /// One-shot startup pipe created by the client.
        #[arg(long)]
        startup_pipe: String,
        /// Canonical launch directory for the app.
        #[arg(long)]
        launch_directory: PathBuf,
        /// Command argv to execute.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
}

/// A parse outcome that should terminate the program before execution.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "handoff struct read directly by the in-crate binary and tests"
)]
pub struct EarlyExit {
    /// The rendered message (help text or error) to print.
    pub output: String,
    /// `Ok` for help or usage text, `Err` for a parse error.
    ///
    /// Missing subcommand or argument is success because clap's implicit help
    /// for that case is the usage text the user asked to see, not a failed
    /// command.
    pub status: Result<(), ()>,
}

impl EarlyExit {
    fn from_clap(error: &clap::Error) -> Self {
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
    /// Parses an argument vector into the typed CLI.
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
        let command = match self.command {
            CliCommand::Run { command } => Command::Run { command },
            CliCommand::Resume { id } => Command::Resume {
                id: id.map(SessionId::new),
            },
            CliCommand::List => Command::List,
            CliCommand::Kill { id } => Command::Kill {
                id: SessionId::new(id),
            },
            CliCommand::Supervisor {
                startup_pipe,
                launch_directory,
                command,
            } => Command::Supervisor {
                startup_pipe,
                launch_directory,
                command,
            },
        };
        RunInput {
            verbose: self.verbose,
            store_root: self.store_root,
            command,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> RunInput {
        Cli::from_args(&["dure"], args).unwrap().into_input()
    }

    #[test]
    fn parse_run_after_double_dash() {
        let input = parse(&["run", "--", "copilot.exe", "--foo"]);
        assert_eq!(
            input.command,
            Command::Run {
                command: vec!["copilot.exe".to_string(), "--foo".to_string()],
            }
        );
    }

    #[test]
    fn parse_resume_without_id() {
        let input = parse(&["resume"]);
        assert_eq!(input.command, Command::Resume { id: None });
    }

    #[test]
    fn parse_resume_with_id() {
        let input = parse(&["resume", "--id", "3"]);
        assert_eq!(
            input.command,
            Command::Resume {
                id: SessionId::from_u32(3),
            }
        );
    }

    #[test]
    fn parse_list() {
        assert_eq!(parse(&["list"]).command, Command::List);
    }

    #[test]
    fn parse_kill_requires_id() {
        Cli::from_args(&["dure"], &["kill"]).unwrap_err();
        let input = parse(&["kill", "--id", "2"]);
        assert_eq!(
            input.command,
            Command::Kill {
                id: SessionId::from_u32(2).unwrap(),
            }
        );
    }

    #[test]
    fn parse_verbose_and_store_root() {
        let input = parse(&["--verbose", "--store-root", r"C:\tmp", "list"]);
        assert!(input.verbose);
        assert_eq!(
            input.store_root.as_deref(),
            Some(std::path::Path::new(r"C:\tmp"))
        );
    }

    #[test]
    fn help_is_early_exit_success() {
        let err = Cli::from_args(&["dure"], &["--help"]).unwrap_err();
        assert!(err.status.is_ok());
        assert!(err.output.contains("dure"));
    }
}
