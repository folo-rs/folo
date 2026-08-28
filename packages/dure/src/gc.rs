//! Drop session records only when the recorded supervisor process is gone.

use ohno::AppError;

use crate::pal::processes::{ProcessLiveness, Processes};
use crate::pal::session_store::SessionStore;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;
use crate::{InspectProcessError, SessionNotFoundError, StoreError};

/// Lists live sessions, deleting records whose supervisor process is gone.
///
/// Id claims left behind by a supervisor that died before publishing are reaped
/// the same way, so a crashed startup does not occupy an id forever.
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
                // Ids are reused, so deleting by id alone can reap a session
                // that claimed this id since `list` read it.
                store
                    .delete_owned_by(record.session_id(), &record.identity())
                    .map_err(|_error| StoreError::new())?;
            }
            ProcessLiveness::InspectFailed => {
                return Err(InspectProcessError::for_pid(record.supervisor_pid).into());
            }
        }
    }
    reap_orphan_reservations(store, processes)?;
    Ok(live)
}

/// Deletes id claims whose owner is gone.
///
/// An unreadable owner is left alone for the same reason a record is: only a
/// confirmed dead process proves the claim will never be published.
fn reap_orphan_reservations(
    store: &impl SessionStore,
    processes: &impl Processes,
) -> Result<(), AppError> {
    let reservations = store
        .list_reservations()
        .map_err(|_error| StoreError::new())?;
    for (id, owner) in reservations {
        if processes.probe(&owner) == ProcessLiveness::Dead {
            store
                .delete_owned_by(id, &owner)
                .map_err(|_error| StoreError::new())?;
        }
    }
    Ok(())
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
            store
                .delete_owned_by(id, &record.identity())
                .map_err(|_error| StoreError::new())?;
            Err(SessionNotFoundError::for_id(id).into())
        }
        ProcessLiveness::InspectFailed => {
            Err(InspectProcessError::for_pid(record.supervisor_pid).into())
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn drops_dead_and_keeps_live() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let live_id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
        let dead_id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
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
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn inspect_failure_keeps_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
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
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn require_live_session_does_not_inspect_other_records() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let live_id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
        let dead_id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
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

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn require_live_session_reaps_a_dead_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
        store.publish(&record(id.get(), 11, 101)).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Dead);

        let error = require_live_session(&store, &processes, id).unwrap_err();
        assert!(error.find_source::<SessionNotFoundError>().is_some());
        assert!(store.read(id).unwrap().is_none());
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn reaps_a_reservation_whose_owner_is_gone() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let orphan = store.allocate_id(&ProcessIdentity::for_test(12)).unwrap();

        let mut processes = MockProcesses::new();
        processes.expect_probe().returning(|_| ProcessLiveness::Dead);

        assert!(live_sessions(&store, &processes).unwrap().is_empty());
        assert!(store.list_reservations().unwrap().is_empty());
        // The id is free again, so the next claim takes it.
        assert_eq!(
            store.allocate_id(&ProcessIdentity::for_test(13)).unwrap(),
            orphan
        );
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn keeps_a_reservation_whose_owner_is_still_initializing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let owner = ProcessIdentity::for_test(12);
        let claimed = store.allocate_id(&owner).unwrap();

        let mut processes = MockProcesses::new();
        processes.expect_probe().returning(|_| ProcessLiveness::Live);

        assert!(live_sessions(&store, &processes).unwrap().is_empty());
        assert_eq!(store.list_reservations().unwrap(), vec![(claimed, owner)]);
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn an_unreadable_reservation_owner_is_left_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let owner = ProcessIdentity::for_test(12);
        store.allocate_id(&owner).unwrap();

        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::InspectFailed);

        // An unreadable owner is not a confirmed death, so the claim stays and
        // reaping reports success.
        assert!(live_sessions(&store, &processes).unwrap().is_empty());
        assert_eq!(store.list_reservations().unwrap().len(), 1);
    }
}
