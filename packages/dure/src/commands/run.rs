//! `dure run`.

use std::path::PathBuf;

use ohno::AppError;

use crate::attach::attach;
use crate::constants::SUPERVISOR_COMMAND;
use crate::pal::error::PalErrorKind;
use crate::pal::local_console::LocalConsole;
use crate::pal::processes::{Processes, SupervisorSpawn};
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::types::Outcome;
use crate::{
    BreakawayDeniedError, CanonicalizeError, CurrentDirectoryError, EmptyCommandError,
    NoConsoleError, PalFailedError, StartupFailedError, StoreError,
};

/// Start a new session, spawn the supervisor, and attach.
pub(crate) fn execute<S, P, T, C>(
    store: &S,
    processes: &P,
    transport: &T,
    console: &C,
    command: Vec<String>,
    store_root: Option<PathBuf>,
) -> Result<Outcome, AppError>
where
    S: SessionStore,
    P: Processes,
    T: Transport + Clone + Send + Sync + 'static,
    C: LocalConsole + Clone + Send + Sync + 'static,
{
    if command.is_empty() {
        return Err(EmptyCommandError::new().into());
    }
    if !console.has_console() {
        return Err(NoConsoleError::new().into());
    }

    let cwd = store
        .current_dir()
        .map_err(|_error| CurrentDirectoryError::new())?;
    let launch_directory = store
        .canonicalize(&cwd)
        .map_err(|_error| CanonicalizeError::new(cwd))?;

    let nonce = processes.random_nonce();
    let startup_pipe = transport.pipe_name(&format!("startup-{nonce}"));
    let listener = transport
        .listen(&startup_pipe)
        .map_err(|_error| StartupFailedError::new())?;

    let exe = processes
        .current_exe()
        .map_err(|_error| PalFailedError::new())?;
    let mut args = vec![
        SUPERVISOR_COMMAND.to_string(),
        "--startup-pipe".to_string(),
        startup_pipe,
        "--launch-directory".to_string(),
        launch_directory.to_string_lossy().into_owned(),
    ];
    if let Some(root) = store_root {
        args.push("--store-root".to_string());
        args.push(root.to_string_lossy().into_owned());
    }
    args.push("--".to_string());
    args.extend(command);

    processes
        .spawn_supervisor(&SupervisorSpawn { exe, args })
        .map_err(|error| match error.kind() {
            PalErrorKind::BreakawayDenied => AppError::from(BreakawayDeniedError::new()),
            _ => AppError::from(StartupFailedError::new()),
        })?;

    let conn = transport
        .accept(listener)
        .map_err(|_error| StartupFailedError::new())?;
    let Ok(Message::StartupOk { session_id }) = transport.recv(conn) else {
        transport.disconnect(conn);
        return Err(StartupFailedError::new().into());
    };
    transport.disconnect(conn);

    let record = store
        .read(session_id)
        .map_err(|_error| StoreError::new())?
        .ok_or_else(StartupFailedError::new)?;
    attach(transport, console, &record.pipe_name, session_id)
}

#[cfg(test)]
mod tests {
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::processes::MockProcesses;
    use crate::pal::session_store::FsSessionStore;
    use crate::pal::transport::MemoryTransport;

    use super::*;

    #[test]
    fn empty_command_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        let transport = MemoryTransport::new();
        let console = LocalConsoleFacade::from_mock(MockLocalConsole::new());
        execute(&store, &processes, &transport, &console, Vec::new(), None).unwrap_err();
    }

    #[test]
    fn no_console_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        let transport = MemoryTransport::new();
        let mut console = MockLocalConsole::new();
        console.expect_has_console().return_const(false);
        let console = LocalConsoleFacade::from_mock(console);
        execute(
            &store,
            &processes,
            &transport,
            &console,
            vec!["app.exe".to_string()],
            None,
        )
        .unwrap_err();
    }
}
