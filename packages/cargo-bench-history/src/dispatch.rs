//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

use std::path::PathBuf;

use jiff::Timestamp;

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
    run_with_overrides(command, None, None, None).await
}

/// Executes a parsed command, overriding the cargo target root (for the
/// `run`/`backfill` harvest), the benchmark command those subcommands invoke, and
/// the analysis clock anchor.
///
/// This exists so end-to-end tests can drive the full `run`/`backfill` flow
/// against a mock benchmark program instead of `cargo bench`, without mutating
/// the process environment, and can pin a deterministic `now` for the analysis
/// default `--since` window. Production code calls [`run`], which passes `None`
/// for all three and uses `cargo bench` with the resolved target root and the
/// wall clock.
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
    now: Option<Timestamp>,
) -> Result<RunOutcome, RunError> {
    match command {
        Command::Run(options) => commands::run(options, target_root, bench_command).await,
        Command::Install(options) => commands::install(options).await,
        Command::Analyze(options) => commands::analyze(options, now).await,
        Command::List(options) => commands::list(options, now).await,
        Command::Clean(options) => commands::clean(options).await,
        Command::Backfill(options) => commands::backfill(options, bench_command).await,
        Command::Bless(options) => commands::bless(options, now).await,
        Command::Unbless(options) => commands::unbless(options).await,
    }
}
