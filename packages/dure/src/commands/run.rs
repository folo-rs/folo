//! `dure run`.

use std::path::PathBuf;

use ohno::AppError;

use crate::attach::attach;
use crate::constants::{CONNECT_TIMEOUT, STARTUP_TIMEOUT, SUPERVISOR_COMMAND};
use crate::durability::Durability;
use crate::pal::error::PalErrorKind;
use crate::pal::local_console::LocalConsole;
use crate::pal::processes::{Processes, SupervisorSpawn};
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::path_display::display_path;
use crate::protocol::Message;
use crate::session_id::SessionId;
use crate::trace::{Trace, trace};
use crate::types::Outcome;
use crate::{
    AttachFailedError, BreakawayDeniedError, CanonicalizeError, CurrentDirectoryError,
    EmptyCommandError, NoConsoleError, PalFailedError, StartupFailedError, StoreError,
};

/// Said when the session cannot outlive the process that launched it.
///
/// Ref: docs/implementation.md, "Job breakaway".
const TIED_TO_LAUNCHER_WARNING: &str = "Warning: this session belongs to a Windows job object that will end it when the launcher exits, so it will not survive a disconnect. Launch dure.exe directly instead of through a wrapper such as `cargo run`.";

/// Start a new session, spawn the supervisor, and attach.
pub(crate) fn execute<S, P, T, C>(
    store: &S,
    processes: &P,
    transport: &T,
    console: &C,
    command: Vec<String>,
    store_root: Option<PathBuf>,
    trace: Trace,
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
    trace!(trace, "app to run: {}", command.join(" "));

    let cwd = store
        .current_dir()
        .map_err(|_error| CurrentDirectoryError::new())?;
    let launch_directory = store
        .canonicalize(&cwd)
        .map_err(|_error| CanonicalizeError::new(cwd))?;
    // Auto-detect matches on this canonicalized form, so it is what a later
    // `dure resume` in this directory will compare against.
    trace!(
        trace,
        "launch directory: {} (auto-detect will match a resume from here)",
        display_path(&launch_directory)
    );

    let nonce = processes.random_nonce();
    let startup_pipe = transport.pipe_name(&format!("startup-{nonce}"));
    trace!(
        trace,
        "listening on {startup_pipe} for the supervisor to report in"
    );
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

    trace!(
        trace,
        "spawning the supervisor: {} {}",
        display_path(&exe),
        args.join(" ")
    );
    processes
        .spawn_supervisor(&SupervisorSpawn { exe, args })
        .map_err(|error| match error.kind() {
            PalErrorKind::BreakawayDenied => AppError::from(BreakawayDeniedError::new()),
            _ => AppError::from(StartupFailedError::new()),
        })?;

    // Initialization gets its own full deadline after this connection is
    // established.
    let conn = match transport.accept_timeout(listener, CONNECT_TIMEOUT) {
        Ok(conn) => conn,
        Err(_error) => {
            transport.close_listener(listener);
            return Err(StartupFailedError::new().into());
        }
    };
    transport.close_listener(listener);

    let response = transport.recv_timeout(conn, STARTUP_TIMEOUT);
    let Ok(Message::StartupOk {
        session_id,
        durability,
    }) = response
    else {
        transport.disconnect(conn);
        return Err(StartupFailedError::new().into());
    };
    if transport.send(conn, &Message::StartupCommit).is_err() {
        transport.disconnect(conn);
        return Err(StartupFailedError::new().into());
    }
    trace!(
        trace,
        "supervisor reported in as session {session_id}, durability {}",
        durability_note(durability)
    );
    if durability == Durability::TiedToLauncher {
        // The supervisor discovers this about itself but has no console
        // to say it on. Ref: docs/implementation.md, "Job breakaway".
        eprintln!("{TIED_TO_LAUNCHER_WARNING}");
    }
    // The supervisor reads this connection as the signal that an attach is
    // still on its way, and holds a session whose app exits immediately open
    // until it arrives. So it stays up for as long as this run intends to
    // attach. Ref: docs/implementation.md, "Process split".
    let outcome = attach_to(store, transport, console, session_id, trace);
    transport.disconnect(conn);
    outcome
}

// Trace wording is not a behavioral contract; the warning that follows a
// tied-to-launcher session is.
#[cfg_attr(test, mutants::skip)]
fn durability_note(durability: Durability) -> &'static str {
    match durability {
        Durability::Durable => "survives this terminal",
        Durability::TiedToLauncher => "tied to the launcher, so it will not survive",
    }
}

/// Read the published record and hand the console over to the session.
fn attach_to<S, T, C>(
    store: &S,
    transport: &T,
    console: &C,
    session_id: SessionId,
    trace: Trace,
) -> Result<Outcome, AppError>
where
    S: SessionStore,
    T: Transport + Clone + Send + Sync + 'static,
    C: LocalConsole + Clone + Send + Sync + 'static,
{
    let record = store
        .read(session_id)
        .map_err(|_error| StoreError::new())?
        .ok_or_else(|| AttachFailedError::for_id(session_id))?;
    trace!(
        trace,
        "attaching to session {session_id} on {}", record.pipe_name
    );
    attach(transport, console, &record.pipe_name, session_id)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::pal::error::PalError;
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::processes::MockProcesses;
    use crate::pal::session_store::{FsSessionStore, MockSessionStore};
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
        execute(
            &store,
            &processes,
            &transport,
            &console,
            Vec::new(),
            None,
            Trace::default(),
        )
        .unwrap_err();
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
            Trace::default(),
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
            Trace::default(),
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
            Trace::default(),
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn a_supervisor_that_does_not_connect_is_a_startup_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let transport = MemoryTransport::new();
        transport.timeout_next_accept();
        let mut processes = MockProcesses::new();
        processes
            .expect_random_nonce()
            .returning(|| "nonce".to_string());
        processes
            .expect_current_exe()
            .returning(|| Ok(PathBuf::from("dure.exe")));
        processes.expect_spawn_supervisor().returning(|_| {
            Ok(ProcessIdentity {
                pid: 10,
                creation_time: 100,
            })
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
            Trace::default(),
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
        assert!(error.find_source::<StartupFailedError>().is_some());
        let error = transport
            .connect(&transport.pipe_name("startup-nonce"), CONNECT_TIMEOUT)
            .unwrap_err();
        assert_eq!(error.kind(), PalErrorKind::Timeout);
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
                let conn = transport.connect(&pipe, CONNECT_TIMEOUT).unwrap();
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
            Trace::default(),
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn a_supervisor_that_disconnects_after_startup_ok_is_a_startup_error() {
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
                let conn = transport.connect(&pipe, CONNECT_TIMEOUT).unwrap();
                transport
                    .send(
                        conn,
                        &Message::StartupOk {
                            session_id: SessionId::MIN,
                            durability: Durability::Durable,
                        },
                    )
                    .unwrap();
                transport.disconnect(conn);
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
            Trace::default(),
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
        assert_eq!(transport.startup_commit_count(), 0);
    }

    /// Drives `execute` through a successful startup handshake against a
    /// supervisor stand-in that reports `durability`, and fails the store read
    /// that follows so the run ends without a live session to attach to.
    fn execute_past_startup(durability: Durability) -> AppError {
        let transport = MemoryTransport::new();
        let mut store = MockSessionStore::new();
        store
            .expect_current_dir()
            .returning(|| Ok(PathBuf::from("cwd")));
        store
            .expect_canonicalize()
            .returning(|path| Ok(path.to_path_buf()));
        store
            .expect_read()
            .returning(|_| Err(PalError::new(PalErrorKind::Other)));
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
                let conn = transport.connect(&pipe, CONNECT_TIMEOUT).unwrap();
                transport
                    .send(
                        conn,
                        &Message::StartupOk {
                            session_id: SessionId::MIN,
                            durability,
                        },
                    )
                    .unwrap();
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
            Trace::default(),
        )
        .unwrap_err();
        assert_eq!(transport.startup_commit_count(), 1);
        error
    }

    #[test]
    fn a_started_session_is_looked_up_in_the_store() {
        let error = execute_past_startup(Durability::Durable);
        assert!(error.find_source::<StoreError>().is_some());
    }

    #[test]
    fn a_session_tied_to_the_launcher_still_starts() {
        let error = execute_past_startup(Durability::TiedToLauncher);
        assert!(error.find_source::<StoreError>().is_some());
    }
}
