//! `dure list`.

use ohno::AppError;

use crate::gc::live_sessions;
use crate::list_fmt::format_list;
use crate::pal::processes::Processes;
use crate::pal::session_store::SessionStore;

/// Print live sessions.
pub(crate) fn execute(
    store: &impl SessionStore,
    processes: &impl Processes,
) -> Result<(), AppError> {
    let live = live_sessions(store, processes)?;
    println!("{}", format_list(&live));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pal::processes::{MockProcesses, ProcessLiveness};
    use crate::pal::session_store::{FsSessionStore, SessionStore};
    use crate::session_record::SessionRecord;

    #[test]
    fn empty_store_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        execute(&store, &processes).unwrap();
    }

    #[test]
    fn live_session_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id().unwrap();
        store
            .publish(&SessionRecord {
                id: id.get(),
                supervisor_pid: 10,
                supervisor_creation_time: 100,
                pipe_name: "pipe".to_string(),
                launch_directory: dir.path().to_path_buf(),
                command: vec!["app.exe".to_string()],
                started_at_unix_ms: 1,
                attached: false,
            })
            .unwrap();
        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Live);
        execute(&store, &processes).unwrap();
    }
}
