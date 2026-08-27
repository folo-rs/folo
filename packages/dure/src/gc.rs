//! Drop session records only when the recorded supervisor process is gone.

use ohno::AppError;

use crate::pal::processes::{ProcessLiveness, Processes};
use crate::pal::session_store::SessionStore;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;
use crate::{InspectProcessError, SessionNotFoundError, StoreError};

/// Lists live sessions, deleting records whose supervisor process is gone.
///
/// A matching running process is kept even if a later connect times out.
/// Failure to inspect a process is an error and does not delete the record.
pub(crate) fn live_sessions(
    store: &impl SessionStore,
    processes: &impl Processes,
) -> Result<Vec<SessionRecord>, AppError> {
    let records = store.list().map_err(|_error| StoreError::new())?;
    let mut live = Vec::new();
    for record in records {
        match processes.probe(&record.identity()) {
            ProcessLiveness::Live => live.push(record),
            ProcessLiveness::Dead => {
                store
                    .delete(record.session_id())
                    .map_err(|_error| StoreError::new())?;
            }
            ProcessLiveness::InspectFailed => {
                return Err(InspectProcessError::for_pid(record.supervisor_pid).into());
            }
        }
    }
    Ok(live)
}

/// Reads and probes only `id`. Unrelated records are not inspected.
pub(crate) fn require_live_session(
    store: &impl SessionStore,
    processes: &impl Processes,
    id: SessionId,
) -> Result<SessionRecord, AppError> {
    let Some(record) = store.read(id).map_err(|_error| StoreError::new())? else {
        return Err(SessionNotFoundError::for_id(id).into());
    };
    match processes.probe(&record.identity()) {
        ProcessLiveness::Live => Ok(record),
        ProcessLiveness::Dead => {
            store.delete(id).map_err(|_error| StoreError::new())?;
            Err(SessionNotFoundError::for_id(id).into())
        }
        ProcessLiveness::InspectFailed => {
            Err(InspectProcessError::for_pid(record.supervisor_pid).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::pal::processes::MockProcesses;
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
    fn drops_dead_and_keeps_live() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let live_id = store.allocate_id().unwrap();
        let dead_id = store.allocate_id().unwrap();
        store.publish(&record(live_id.get(), 10, 100)).unwrap();
        store.publish(&record(dead_id.get(), 11, 101)).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|identity: &ProcessIdentity| {
                if identity.pid == 10 {
                    ProcessLiveness::Live
                } else {
                    ProcessLiveness::Dead
                }
            });

        let live = live_sessions(&store, &processes).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live.first().expect("one live session").id, live_id.get());
        assert!(store.read(dead_id).unwrap().is_none());
        assert!(store.read(live_id).unwrap().is_some());
    }

    #[test]
    fn inspect_failure_keeps_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id().unwrap();
        store.publish(&record(id.get(), 10, 100)).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::InspectFailed);

        live_sessions(&store, &processes).unwrap_err();
        assert!(store.read(id).unwrap().is_some());
        _ = SessionId::MIN;
    }

    #[test]
    fn require_live_session_does_not_inspect_other_records() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let live_id = store.allocate_id().unwrap();
        let dead_id = store.allocate_id().unwrap();
        store.publish(&record(live_id.get(), 10, 100)).unwrap();
        store.publish(&record(dead_id.get(), 11, 101)).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .times(1)
            .withf(|identity: &ProcessIdentity| identity.pid == 10)
            .returning(|_| ProcessLiveness::Live);

        let found = require_live_session(&store, &processes, live_id).unwrap();
        assert_eq!(found.id, live_id.get());
        assert!(store.read(dead_id).unwrap().is_some());
    }
}
