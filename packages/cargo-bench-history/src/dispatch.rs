//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

use crate::commands;
use crate::{Command, RunError, RunOutcome};

/// Executes a parsed command.
///
/// The `run` and `analyze` handlers are asynchronous (they drive IO over the
/// configured storage); `install` remains a synchronous stub that reports
/// `RunOutcome::NotImplemented` until a later iteration fills it in.
///
/// # Errors
///
/// Returns a `RunError` if a command fails (for example, a configuration or
/// storage error).
#[doc(hidden)]
pub async fn run(command: &Command) -> Result<RunOutcome, RunError> {
    match command {
        Command::Run(options) => commands::run(options).await,
        Command::Install(options) => commands::install(options),
        Command::Analyze(options) => commands::analyze(options).await,
    }
}
