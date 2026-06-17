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
    run_with_overrides(command, None, None).await
}

/// Executes a parsed command, overriding both the cargo target root (for the
/// `run`/`backfill` harvest) and the benchmark command those subcommands invoke.
///
/// This exists so end-to-end tests can drive the full `run`/`backfill` flow
/// against a mock benchmark program instead of `cargo bench`, without mutating
/// the process environment. Production code calls [`run`], which passes `None`
/// for both and uses `cargo bench` with the resolved target root.
///
/// # Errors
///
/// Returns a `RunError` if a command fails (for example, a configuration or
/// storage error).
#[doc(hidden)]
pub async fn run_with_overrides(
    command: &Command,
    target_root: Option<PathBuf>,
    bench_command: Option<Vec<String>>,
) -> Result<RunOutcome, RunError> {
    match command {
        Command::Run(options) => commands::run(options, target_root, bench_command).await,
        Command::Install(options) => commands::install(options).await,
        Command::Analyze(options) => commands::analyze(options).await,
        Command::Backfill(options) => commands::backfill(options, bench_command).await,
    }
}
