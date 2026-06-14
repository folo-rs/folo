//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

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
    match command {
        Command::Run(options) => commands::run(options).await,
        Command::Install(options) => commands::install(options).await,
        Command::Analyze(options) => commands::analyze(options).await,
    }
}
