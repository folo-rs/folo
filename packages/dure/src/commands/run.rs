//! `dure run`.

use std::path::PathBuf;
use std::thread;

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

    thread::spawn({
        let transport = transport.clone();
        move || {
            thread::sleep(crate::constants::CONNECT_TIMEOUT);
            transport.close_listener(listener);
        }
    });

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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::pal::error::PalError;
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::processes::MockProcesses;
    use crate::pal::session_store::FsSessionStore;
    use crate::pal::transport::MemoryTransport;
    use crate::session_record::ProcessIdentity;

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn empty_command_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        let transport = MemoryTransport::new();
        let console = LocalConsoleFacade::from_mock(MockLocalConsole::new());
        execute(&store, &processes, &transport, &console, Vec::new(), None).unwrap_err();
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
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

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn breakaway_denied_is_breakaway_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let mut processes = MockProcesses::new();
        processes
            .expect_random_nonce()
            .returning(|| "nonce".to_string());
        processes
            .expect_current_exe()
            .returning(|| Ok(PathBuf::from("dure.exe")));
        processes
            .expect_spawn_supervisor()
            .returning(|_| Err(PalError::new(PalErrorKind::BreakawayDenied)));
        let transport = MemoryTransport::new();
        let mut console = MockLocalConsole::new();
        console.expect_has_console().return_const(true);
        let console = LocalConsoleFacade::from_mock(console);
        let error = execute(
            &store,
            &processes,
            &transport,
            &console,
            vec!["app.exe".to_string()],
            None,
        )
        .unwrap_err();
        assert!(error.find_source::<BreakawayDeniedError>().is_some());
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn spawn_failure_is_startup_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let mut processes = MockProcesses::new();
        processes
            .expect_random_nonce()
            .returning(|| "nonce".to_string());
        processes
            .expect_current_exe()
            .returning(|| Ok(PathBuf::from("dure.exe")));
        processes
            .expect_spawn_supervisor()
            .returning(|_| Err(PalError::new(PalErrorKind::Other)));
        let transport = MemoryTransport::new();
        let mut console = MockLocalConsole::new();
        console.expect_has_console().return_const(true);
        let console = LocalConsoleFacade::from_mock(console);
        let error = execute(
            &store,
            &processes,
            &transport,
            &console,
            vec!["app.exe".to_string()],
            None,
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn a_supervisor_that_reports_failure_is_a_startup_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let transport = MemoryTransport::new();
        let mut processes = MockProcesses::new();
        processes
            .expect_random_nonce()
            .returning(|| "nonce".to_string());
        processes
            .expect_current_exe()
            .returning(|| Ok(PathBuf::from("dure.exe")));
        processes.expect_spawn_supervisor().returning({
            let transport = transport.clone();
            move |_| {
                let pipe = transport.pipe_name("startup-nonce");
                let conn = transport
                    .connect(&pipe, crate::constants::CONNECT_TIMEOUT)
                    .unwrap();
                transport.send(conn, &Message::StartupErr).unwrap();
                Ok(ProcessIdentity {
                    pid: 10,
                    creation_time: 100,
                })
            }
        });
        let mut console = MockLocalConsole::new();
        console.expect_has_console().return_const(true);
        let console = LocalConsoleFacade::from_mock(console);

        let error = execute(
            &store,
            &processes,
            &transport,
            &console,
            vec!["app.exe".to_string()],
            None,
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
    }
}
