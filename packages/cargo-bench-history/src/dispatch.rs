//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

use std::path::PathBuf;

use crate::commands;
use crate::{Command, RunError, RunOutcome};

/// Executes a parsed command.
///
/// Every handler is asynchronous: `run` and `analyze` drive IO over the
/// configured storage, and `install` writes a configuration file.
///
/// # Errors
///
/// Returns a `RunError` if a command fails (for example, a configuration or
/// storage error).
#[doc(hidden)]
pub async fn run(command: &Command) -> Result<RunOutcome, RunError> {
    run_with_target_root(command, None).await
}

/// Executes a parsed command with an explicit cargo target root for the `run`
/// harvest (other commands ignore it).
///
/// This exists so end-to-end tests can point the harvest at a fixture `target/`
/// tree without mutating the process-wide `CARGO_TARGET_DIR`. Production code
/// calls [`run`], which passes `None` and resolves the root from the
/// environment.
///
/// # Errors
///
/// Returns a `RunError` if a command fails (for example, a configuration or
/// storage error).
#[doc(hidden)]
pub async fn run_with_target_root(
    command: &Command,
    target_root: Option<PathBuf>,
) -> Result<RunOutcome, RunError> {
    match command {
        Command::Run(options) => commands::run_with_target_root(options, target_root).await,
        Command::Install(options) => commands::install(options).await,
        Command::Analyze(options) => commands::analyze(options).await,
        Command::Backfill(options) => commands::backfill(options).await,
    }
}
