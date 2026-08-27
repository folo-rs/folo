//! `dure resume`.

use ohno::AppError;

use crate::attach::attach;
use crate::detect::{DetectOutcome, auto_detect};
use crate::gc::live_sessions;
use crate::list_fmt::format_list;
use crate::pal::local_console::LocalConsole;
use crate::pal::processes::Processes;
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;
use crate::types::Outcome;
use crate::{
    CanonicalizeError, CurrentDirectoryError, NoLiveSessionsError, PromptFailedError,
    SessionNotFoundError, parse_prompted_id,
};

/// Attach using auto-detect or an explicit id.
pub(crate) fn execute<S, P, T, C>(
    store: &S,
    processes: &P,
    transport: &T,
    console: &C,
    id: Option<SessionId>,
    verbose: bool,
) -> Result<Outcome, AppError>
where
    S: SessionStore,
    P: Processes,
    T: Transport + Clone + Send + Sync + 'static,
    C: LocalConsole + Clone + Send + Sync + 'static,
{
    let record = match id {
        Some(id) => crate::gc::require_live_session(store, processes, id)?,
        None => {
            let live = live_sessions(store, processes)?;
            let id = resolve_auto(store, console, &live, verbose)?;
            live.into_iter()
                .find(|record| record.session_id() == id)
                .ok_or_else(|| AppError::from(SessionNotFoundError::for_id(id)))?
        }
    };
    attach(transport, console, &record.pipe_name, record.session_id())
}

fn resolve_auto<S, C>(
    store: &S,
    console: &C,
    live: &[SessionRecord],
    verbose: bool,
) -> Result<SessionId, AppError>
where
    S: SessionStore,
    C: LocalConsole,
{
    let cwd = store
        .current_dir()
        .map_err(|_error| CurrentDirectoryError::new())?;
    let cwd = store
        .canonicalize(&cwd)
        .map_err(|_error| CanonicalizeError::new(cwd))?;
    match auto_detect(live, &cwd, verbose) {
        DetectOutcome::None => Err(NoLiveSessionsError::new().into()),
        DetectOutcome::Unique(id) => Ok(id),
        DetectOutcome::Ambiguous(sessions) => {
            println!("{}", format_list(&sessions));
            if !console.stdin_is_terminal() {
                return Err(PromptFailedError::new().into());
            }
            let line = console
                .read_prompt_line()
                .map_err(|_error| PromptFailedError::new())?;
            parse_prompted_id(&line).map_err(AppError::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::processes::MockProcesses;
    use crate::pal::session_store::FsSessionStore;
    use crate::pal::transport::MemoryTransport;
    use crate::session_id::SessionId;

    #[test]
    fn no_live_sessions_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        let transport = MemoryTransport::new();
        let console = LocalConsoleFacade::from_mock(MockLocalConsole::new());
        execute(&store, &processes, &transport, &console, None, false).unwrap_err();
    }

    #[test]
    fn missing_explicit_id_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        let transport = MemoryTransport::new();
        let console = LocalConsoleFacade::from_mock(MockLocalConsole::new());
        let id = SessionId::from_u32(9).unwrap();
        execute(&store, &processes, &transport, &console, Some(id), false).unwrap_err();
    }
}
