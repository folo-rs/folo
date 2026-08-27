//! `dure kill --id`.

use ohno::AppError;

use crate::KillFailedError;
use crate::gc::require_live_session;
use crate::pal::processes::Processes;
use crate::pal::session_store::SessionStore;
use crate::session_id::SessionId;

/// Abruptly terminate the recorded supervisor process.
pub(crate) fn execute(
    store: &impl SessionStore,
    processes: &impl Processes,
    id: SessionId,
) -> Result<(), AppError> {
    let record = require_live_session(store, processes, id)?;
    processes
        .terminate(&record.identity())
        .map_err(|_error| KillFailedError::for_id(id))?;
    store
        .delete(id)
        .map_err(|_error| crate::StoreError::new())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::pal::error::{PalError, PalErrorKind};
    use crate::pal::processes::{MockProcesses, ProcessLiveness};
    use crate::pal::session_store::{FsSessionStore, SessionStore};
    use crate::session_id::SessionId;
    use crate::session_record::{ProcessIdentity, SessionRecord};

    fn record(id: u32, pid: u32, creation: u64) -> SessionRecord {
        SessionRecord {
            id,
            supervisor_pid: pid,
            supervisor_creation_time: creation,
            pipe_name: format!("pipe-{id}"),
            launch_directory: PathBuf::from("/work"),
            command: vec!["app.exe".to_string()],
            started_at_unix_ms: 1,
            attached: false,
        }
    }

    #[test]
    fn missing_id_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let mut processes = MockProcesses::new();
        processes.expect_probe().never();
        let id = SessionId::from_u32(1).unwrap();
        execute(&store, &processes, id).unwrap_err();
    }

    #[test]
    fn terminates_recorded_identity_and_deletes() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id().unwrap();
        store.publish(&record(id.get(), 10, 100)).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Live);
        processes
            .expect_terminate()
            .withf(|identity: &ProcessIdentity| identity.pid == 10 && identity.creation_time == 100)
            .returning(|_| Ok(()));

        execute(&store, &processes, id).unwrap();
        assert!(store.read(id).unwrap().is_none());
    }

    #[test]
    fn pid_reuse_does_not_delete_when_terminate_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id().unwrap();
        store.publish(&record(id.get(), 10, 100)).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Live);
        processes
            .expect_terminate()
            .returning(|_| Err(PalError::new(PalErrorKind::NotFound)));

        execute(&store, &processes, id).unwrap_err();
        assert!(store.read(id).unwrap().is_some());
    }
}
