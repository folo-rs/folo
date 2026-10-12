//! Library entry point that dispatches a parsed [`crate::Invocation`].

use ohno::AppError;

use crate::pal::Pal;
use crate::path_display::display_path;
use crate::supervisor::SessionSpec;
use crate::trace::{Trace, trace};
use crate::wall_clock::unix_now_ms;
use crate::{Command, Invocation, Outcome, PalFailedError, commands};

/// Executes a parsed `dure` invocation.
///
/// # Errors
///
/// Returns an error when a session cannot be started, resumed, listed, or
/// killed, and when an attached client is displaced by a newer attach.
pub fn run(input: &Invocation) -> Result<Outcome, AppError> {
    let pal = Pal::target(input.session_store_root()).map_err(PalFailedError::caused_by)?;
    dispatch(input, &pal)
}

pub(crate) fn dispatch(input: &Invocation, pal: &Pal) -> Result<Outcome, AppError> {
    dispatch_with_clock(input, pal, unix_now_ms)
}

/// Dispatches against an explicit clock source so routing tests stay in-process.
fn dispatch_with_clock(
    input: &Invocation,
    pal: &Pal,
    now: impl Fn() -> u64,
) -> Result<Outcome, AppError> {
    let trace = Trace::new(input.verbose);
    trace!(
        trace,
        "store root: {}",
        input
            .session_store_root()
            .as_deref()
            .map_or_else(|| "per-user default".to_string(), display_path)
    );
    match &input.command {
        Command::Run { command } => commands::run::execute(
            &pal.store,
            &pal.processes,
            &pal.transport,
            &pal.console,
            command,
            input.session_store_root(),
            trace,
        ),
        Command::Resume { id } => commands::resume::execute(
            &pal.store,
            &pal.processes,
            &pal.transport,
            &pal.console,
            *id,
            now(),
            trace,
        ),
        Command::List => {
            commands::list::execute(&pal.store, &pal.processes, now(), trace)?;
            Ok(Outcome::Success)
        }
        Command::Kill { id } => {
            commands::kill::execute(&pal.store, &pal.processes, *id, trace)?;
            Ok(Outcome::Success)
        }
        Command::Supervisor {
            startup_pipe,
            launch_directory,
            command,
        } => commands::supervise::execute(
            &pal.store,
            &pal.processes,
            &pal.transport,
            &pal.pty,
            startup_pipe,
            SessionSpec {
                launch_directory: launch_directory.clone(),
                command: command.clone(),
                started_at_unix_ms: now(),
            },
        ),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::processes::{MockProcesses, ProcessLiveness, ProcessesFacade};
    use crate::pal::pseudoconsole::{MemoryPseudoconsole, PseudoconsoleFacade};
    use crate::pal::session_store::{MockSessionStore, SessionStoreFacade};
    use crate::pal::transport::{MemoryTransport, TransportFacade};
    use crate::protocol::PROTOCOL_VERSION;
    use crate::session_record::{ProcessIdentity, SessionRecord};
    use crate::{AppCommand, SessionId, SessionNotFoundError};

    /// Routes prepared requests with a fixed, irrelevant display timestamp.
    fn dispatch(input: &Invocation, pal: &Pal) -> Result<Outcome, AppError> {
        dispatch_with_clock(input, pal, || 0)
    }

    fn pal_with(store: MockSessionStore, processes: MockProcesses) -> Pal {
        Pal {
            store: SessionStoreFacade::from_mock(store),
            processes: ProcessesFacade::from_mock(processes),
            transport: TransportFacade::from_memory(MemoryTransport::new()),
            console: LocalConsoleFacade::from_mock({
                let mut console = MockLocalConsole::new();
                console.expect_has_console().return_const(true);
                console
            }),
            pty: PseudoconsoleFacade::from_memory(MemoryPseudoconsole::new()),
        }
    }

    fn input(command: Command) -> Invocation {
        Invocation {
            verbose: false,
            store_root: None,
            command,
        }
    }

    #[test]
    fn resume_reaches_the_resume_command() {
        let mut store = MockSessionStore::new();
        store.expect_read().returning(|_| Ok(None));
        let mut processes = MockProcesses::new();
        processes.expect_probe().never();

        let error = dispatch(
            &input(Command::Resume {
                id: Some(SessionId::MIN),
            }),
            &pal_with(store, processes),
        )
        .unwrap_err();

        assert!(error.find_source::<SessionNotFoundError>().is_some());
    }

    #[test]
    fn kill_reports_success_without_an_app_status() {
        let mut store = MockSessionStore::new();
        store.expect_read().returning(|id| {
            Ok(Some(SessionRecord {
                id,
                supervisor: ProcessIdentity {
                    pid: 10_u32.try_into().unwrap(),
                    creation_time: 100,
                },
                pipe_name: "pipe".to_string(),
                launch_directory: PathBuf::from("/work"),
                command: AppCommand::for_test(&["app.exe"]),
                started_at_unix_ms: 1,
                attached: false,
                protocol_version: PROTOCOL_VERSION,
            }))
        });
        store.expect_delete_owned_by().returning(|_, _| Ok(()));
        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Live);
        processes.expect_terminate().returning(|_| Ok(()));

        let outcome = dispatch(
            &input(Command::Kill { id: SessionId::MIN }),
            &pal_with(store, processes),
        )
        .unwrap();

        assert_eq!(outcome, Outcome::Success);
    }
}
