//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

use crate::commands;
use crate::{Command, RunError, RunOutcome};

/// Executes a parsed command.
///
/// The `run` handler is asynchronous (it drives the benchmark engines and IO);
/// `install` and `analyze` remain synchronous stubs that report
/// `RunOutcome::NotImplemented` until later iterations fill them in.
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
        Command::Analyze(options) => commands::analyze(options),
    }
}
