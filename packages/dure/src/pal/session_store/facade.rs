//! Facade over session store implementations.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::pal::error::PalError;
#[cfg(test)]
use crate::pal::session_store::MockSessionStore;
use crate::pal::session_store::{FsSessionStore, SessionStore};
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;

/// Dispatches session-store calls to the filesystem store or a test mock.
#[derive(Clone)]
pub(crate) enum SessionStoreFacade {
    /// Real filesystem store.
    Target(Arc<FsSessionStore>),
    /// Mock store for tests that assert call patterns.
    #[cfg(test)]
    Mock(Arc<MockSessionStore>),
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl fmt::Debug for SessionStoreFacade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Target(_) => f.debug_tuple(stringify!(Target)).finish(),
            #[cfg(test)]
            Self::Mock(_) => f.debug_tuple(stringify!(Mock)).finish(),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl SessionStoreFacade {
    pub(crate) fn target(root: PathBuf) -> Self {
        Self::Target(Arc::new(FsSessionStore::new(root)))
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockSessionStore) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl SessionStore for SessionStoreFacade {
    fn root(&self) -> PathBuf {
        match self {
            Self::Target(store) => store.root(),
            #[cfg(test)]
            Self::Mock(store) => store.root(),
        }
    }

    fn allocate_id(&self) -> Result<SessionId, PalError> {
        match self {
            Self::Target(store) => store.allocate_id(),
            #[cfg(test)]
            Self::Mock(store) => store.allocate_id(),
        }
    }

    fn publish(&self, record: &SessionRecord) -> Result<(), PalError> {
        match self {
            Self::Target(store) => store.publish(record),
            #[cfg(test)]
            Self::Mock(store) => store.publish(record),
        }
    }

    fn read(&self, id: SessionId) -> Result<Option<SessionRecord>, PalError> {
        match self {
            Self::Target(store) => store.read(id),
            #[cfg(test)]
            Self::Mock(store) => store.read(id),
        }
    }

    fn list(&self) -> Result<Vec<SessionRecord>, PalError> {
        match self {
            Self::Target(store) => store.list(),
            #[cfg(test)]
            Self::Mock(store) => store.list(),
        }
    }

    fn delete(&self, id: SessionId) -> Result<(), PalError> {
        match self {
            Self::Target(store) => store.delete(id),
            #[cfg(test)]
            Self::Mock(store) => store.delete(id),
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PalError> {
        match self {
            Self::Target(store) => store.canonicalize(path),
            #[cfg(test)]
            Self::Mock(store) => store.canonicalize(path),
        }
    }

    fn current_dir(&self) -> Result<PathBuf, PalError> {
        match self {
            Self::Target(store) => store.current_dir(),
            #[cfg(test)]
            Self::Mock(store) => store.current_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_mock_dispatches_root() {
        let mut mock = MockSessionStore::new();
        mock.expect_root().returning(|| PathBuf::from("/tmp/dure"));
        let facade = SessionStoreFacade::from_mock(mock);
        assert_eq!(facade.root(), PathBuf::from("/tmp/dure"));
        _ = format!("{facade:?}");
    }
}
