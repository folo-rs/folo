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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::SessionNotFoundError;
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::processes::{MockProcesses, ProcessLiveness, ProcessesFacade};
    use crate::pal::pseudoconsole::{MemoryPseudoconsole, PseudoconsoleFacade};
    use crate::pal::session_store::{MockSessionStore, SessionStoreFacade};
    use crate::pal::transport::{MemoryTransport, TransportFacade};
    use crate::session_id::SessionId;
    use crate::session_record::SessionRecord;

    fn pal_with(store: MockSessionStore, processes: MockProcesses) -> Pal {
        Pal {
            store: SessionStoreFacade::from_mock(store),
            processes: ProcessesFacade::from_mock(processes),
            transport: TransportFacade::from_memory(MemoryTransport::new()),
            console: LocalConsoleFacade::from_mock(MockLocalConsole::new()),
            pty: PseudoconsoleFacade::from_memory(MemoryPseudoconsole::new()),
        }
    }

    fn input(command: Command) -> RunInput {
        RunInput {
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
                id: id.get(),
                supervisor_pid: 10,
                supervisor_creation_time: 100,
                pipe_name: "pipe".to_string(),
                launch_directory: std::path::PathBuf::from("/work"),
                command: vec!["app.exe".to_string()],
                started_at_unix_ms: 1,
                attached: false,
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
