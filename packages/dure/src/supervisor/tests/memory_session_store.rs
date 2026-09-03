#![cfg_attr(coverage_nightly, coverage(off))]

//! In-memory session store for supervisor unit tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::session_store::SessionStore;
use crate::session_id::SessionId;
use crate::session_record::{ProcessIdentity, SessionRecord, StoredSession};

/// Shared stateful session store for supervisor unit tests.
///
/// Supervisor tests exercise synchronization among the process, transport,
/// pseudoconsole, and store boundaries. Keeping records in memory ensures a
/// filesystem flush cannot consume the watchdog intended to diagnose those
/// synchronization paths.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemorySessionStore {
    inner: Arc<MemorySessionStoreInner>,
}

/// Record state and deterministic publish-stall coordination.
#[derive(Debug, Default)]
struct MemorySessionStoreInner {
    records: Mutex<BTreeMap<SessionId, StoredSession>>,
    /// Injects a publication failure after id allocation.
    fail_next_publish: AtomicBool,
    publish_stall: Mutex<PublishStall>,
    publish_stall_changed: Condvar,
}

/// Gate used to park store publications at a known point in a test.
#[derive(Debug, Default)]
struct PublishStall {
    enabled: bool,
    waiting: usize,
}

impl MemorySessionStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn stall_publishes(&self) {
        let mut stall = self.inner.publish_stall.lock().unwrap();
        assert!(!stall.enabled);
        stall.enabled = true;
    }

    pub(crate) fn fail_next_publish(&self) {
        self.inner.fail_next_publish.store(true, Ordering::SeqCst);
    }

    pub(crate) fn wait_for_stalled_publish(&self) {
        let mut stall = self.inner.publish_stall.lock().unwrap();
        assert!(
            stall.enabled,
            "the publish stall must be enabled before waiting"
        );
        while stall.waiting == 0 {
            stall = self.inner.publish_stall_changed.wait(stall).unwrap();
        }
    }

    pub(crate) fn resume_publishes(&self) {
        let mut stall = self.inner.publish_stall.lock().unwrap();
        assert!(stall.enabled);
        stall.enabled = false;
        self.inner.publish_stall_changed.notify_all();
    }

    fn await_publish_permission(&self) {
        let mut stall = self.inner.publish_stall.lock().unwrap();
        if !stall.enabled {
            return;
        }
        stall.waiting = stall.waiting.checked_add(1).unwrap();
        self.inner.publish_stall_changed.notify_all();
        while stall.enabled {
            stall = self.inner.publish_stall_changed.wait(stall).unwrap();
        }
        stall.waiting = stall.waiting.checked_sub(1).unwrap();
    }
}

impl SessionStore for MemorySessionStore {
    fn root(&self) -> PathBuf {
        // Supervisor tests receive prepared paths and never query store path support.
        panic!("supervisor tests do not query the session store root")
    }

    fn allocate_id(&self, owner: &ProcessIdentity) -> Result<SessionId, PalError> {
        let mut records = self.inner.records.lock().unwrap();
        let mut id = SessionId::MIN;
        while records.contains_key(&id) {
            id = id
                .get()
                .checked_add(1)
                .and_then(SessionId::from_u32)
                .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
        }
        records.insert(id, StoredSession::Reserved { owner: *owner });
        Ok(id)
    }

    fn publish(&self, record: &SessionRecord) -> Result<(), PalError> {
        if self.inner.fail_next_publish.swap(false, Ordering::SeqCst) {
            return Err(PalError::new(PalErrorKind::Other));
        }
        self.await_publish_permission();
        self.inner.records.lock().unwrap().insert(
            record.session_id(),
            StoredSession::Published(record.clone()),
        );
        Ok(())
    }

    fn read(&self, id: SessionId) -> Result<Option<SessionRecord>, PalError> {
        let records = self.inner.records.lock().unwrap();
        Ok(match records.get(&id) {
            Some(StoredSession::Published(record)) => Some(record.clone()),
            Some(StoredSession::Reserved { .. }) | None => None,
        })
    }

    fn list(&self) -> Result<Vec<SessionRecord>, PalError> {
        let records = self.inner.records.lock().unwrap();
        Ok(records
            .values()
            .filter_map(|stored| match stored {
                StoredSession::Published(record) => Some(record.clone()),
                StoredSession::Reserved { .. } => None,
            })
            .collect())
    }

    fn list_reservations(&self) -> Result<Vec<(SessionId, ProcessIdentity)>, PalError> {
        let records = self.inner.records.lock().unwrap();
        Ok(records
            .iter()
            .filter_map(|(id, stored)| match stored {
                StoredSession::Reserved { owner } => Some((*id, *owner)),
                StoredSession::Published(_) => None,
            })
            .collect())
    }

    fn delete_owned_by(&self, id: SessionId, owner: &ProcessIdentity) -> Result<(), PalError> {
        let mut records = self.inner.records.lock().unwrap();
        let owned = match records.get(&id) {
            Some(StoredSession::Reserved { owner: current }) => current == owner,
            Some(StoredSession::Published(record)) => record.identity() == *owner,
            None => false,
        };
        if owned {
            records.remove(&id);
        }
        Ok(())
    }

    fn canonicalize(&self, _path: &Path) -> Result<PathBuf, PalError> {
        // Supervisor tests receive prepared paths and never query store path support.
        panic!("supervisor tests do not canonicalize paths through the session store")
    }

    fn current_dir(&self) -> Result<PathBuf, PalError> {
        // Supervisor tests receive prepared paths and never query store path support.
        panic!("supervisor tests do not query the current directory")
    }
}
