//! Hidden supervisor process.

use std::path::PathBuf;

use ohno::AppError;

use crate::pal::processes::Processes;
use crate::pal::pseudoconsole::Pseudoconsole;
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::supervisor::run_supervisor;
use crate::types::Outcome;

/// Run the supervisor role until the app exits.
pub(crate) fn execute<S, P, T, Y>(
    store: &S,
    processes: &P,
    transport: &T,
    pty_host: &Y,
    startup_pipe: &str,
    launch_directory: PathBuf,
    command: Vec<String>,
) -> Result<Outcome, AppError>
where
    S: SessionStore + Clone,
    P: Processes,
    T: Transport + Clone + Send + Sync + 'static,
    Y: Pseudoconsole + Clone + Send + Sync + 'static,
{
    let status = run_supervisor(
        processes,
        store,
        transport,
        pty_host,
        startup_pipe,
        launch_directory,
        command,
    )?;
    Ok(Outcome::AppExit(status))
}
