//! Library entry point that dispatches a parsed [`crate::RunInput`].

use ohno::AppError;

use crate::commands;
use crate::pal::Pal;
use crate::platform::ensure_supported_platform;
use crate::types::{Command, Outcome, RunInput};

/// Executes a parsed `dure` invocation.
///
/// # Errors
///
/// Returns an error when the platform is not Windows, when a session cannot be
/// started, resumed, listed, or killed, or when attach is displaced.
pub fn run(input: &RunInput) -> Result<Outcome, AppError> {
    ensure_supported_platform()?;
    let pal =
        Pal::target(input.store_root.clone()).map_err(|_error| crate::PalFailedError::new())?;
    dispatch(input, &pal)
}

pub(crate) fn dispatch(input: &RunInput, pal: &Pal) -> Result<Outcome, AppError> {
    match &input.command {
        Command::Run { command } => commands::run::execute(
            &pal.store,
            &pal.processes,
            &pal.transport,
            &pal.console,
            command.clone(),
            input.store_root.clone(),
        ),
        Command::Resume { id } => commands::resume::execute(
            &pal.store,
            &pal.processes,
            &pal.transport,
            &pal.console,
            *id,
            input.verbose,
        ),
        Command::List => {
            commands::list::execute(&pal.store, &pal.processes)?;
            Ok(Outcome::Success)
        }
        Command::Kill { id } => {
            commands::kill::execute(&pal.store, &pal.processes, *id)?;
            Ok(Outcome::Success)
        }
        Command::Supervisor {
            startup_pipe,
            launch_directory,
            command,
        } => commands::supervisor::execute(
            &pal.store,
            &pal.processes,
            &pal.transport,
            &pal.pty,
            startup_pipe,
            launch_directory.clone(),
            command.clone(),
        ),
    }
}
