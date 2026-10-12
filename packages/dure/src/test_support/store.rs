//! Narrow adapters for native store scenarios without publishing PAL types.

use std::io;
use std::path::{Path, PathBuf};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::session_store::{self, FsSessionStore, RecordFile, SessionStore, StoredSession};
use crate::pal::store_root::resolve_store_root;
use crate::protocol::PROTOCOL_VERSION;
use crate::session_record::{ProcessIdentity, SessionRecord};
use crate::{AppCommand, SessionId};

/// Drives the production filesystem store from integration scenarios.
#[derive(Clone, Debug)]
pub struct Store(FsSessionStore);

impl Store {
    /// Selects the test-owned store directory.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self(FsSessionStore::new(root))
    }

    /// Claims the next available session id.
    pub fn allocate_id(&self, owner: &Identity) -> io::Result<SessionId> {
        self.0.allocate_id(&owner.0).map_err(io::Error::other)
    }

    /// Attempts installation from a prepared or deliberately missing staging file.
    pub fn install_claim(&self, staging: &Path) -> io::Result<SessionId> {
        self.0.install_claim(staging).map_err(io::Error::other)
    }

    /// Publishes a prepared record.
    pub fn publish(&self, record: &Record) -> io::Result<()> {
        self.0.publish(&record.0).map_err(io::Error::other)
    }

    /// Reads the published record for an id.
    pub fn read(&self, id: SessionId) -> io::Result<Option<Record>> {
        self.0
            .read(id)
            .map(|record| record.map(Record))
            .map_err(io::Error::other)
    }

    /// Lists readable published records.
    pub fn list(&self) -> io::Result<Vec<Record>> {
        self.0
            .list()
            .map(|records| records.into_iter().map(Record).collect())
            .map_err(io::Error::other)
    }

    /// Lists claims that have not been published.
    pub fn list_reservations(&self) -> io::Result<Vec<(SessionId, Identity)>> {
        self.0
            .list_reservations()
            .map(|claims| {
                claims
                    .into_iter()
                    .map(|(id, owner)| (id, Identity(owner)))
                    .collect()
            })
            .map_err(io::Error::other)
    }

    /// Removes an id only when its stored owner matches.
    pub fn delete_owned_by(&self, id: SessionId, owner: &Identity) -> io::Result<()> {
        self.0
            .delete_owned_by(id, &owner.0)
            .map_err(io::Error::other)
    }

    /// Resolves the path through the filesystem boundary.
    pub fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.0.canonicalize(path).map_err(io::Error::other)
    }
}

/// Prepared record with controlled identity and content for native scenarios.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record(SessionRecord);

impl Record {
    /// Uses ordinary fixture values for fields unrelated to storage behavior.
    #[must_use]
    pub fn new(id: SessionId, directory: &Path) -> Self {
        Self(SessionRecord {
            id,
            supervisor: Identity::for_test(1).0,
            pipe_name: "pipe".to_string(),
            launch_directory: directory.to_path_buf(),
            command: AppCommand::from_argv(vec!["app.exe".to_string()]).unwrap(),
            started_at_unix_ms: 1,
            attached: false,
            protocol_version: PROTOCOL_VERSION,
        })
    }

    /// The owner a conditional deletion must match.
    #[must_use]
    pub fn supervisor(&self) -> Identity {
        Identity(self.0.supervisor)
    }

    /// Selects the record's process identity.
    pub fn set_supervisor(&mut self, owner: Identity) {
        self.0.supervisor = owner.0;
    }

    /// Selects the command, including large-content cases.
    pub fn set_command(&mut self, command: AppCommand) {
        self.0.command = command;
    }

    /// Encodes the production storage representation for malformed-placement tests.
    #[must_use]
    pub fn stored_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&StoredSession::Published(self.0.clone())).unwrap()
    }
}

/// A positive supervisor identity, including its process-reuse discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity(ProcessIdentity);

impl Identity {
    /// Uses the supplied positive pid and creation time.
    #[must_use]
    pub fn new(pid: u32, creation_time: u64) -> Self {
        Self(ProcessIdentity {
            pid: pid.try_into().unwrap(),
            creation_time,
        })
    }

    /// Uses a reproducible arbitrary creation time for ownership comparisons.
    #[must_use]
    pub fn for_test(pid: u32) -> Self {
        Self::new(pid, u64::from(pid))
    }
}

/// Holds a native record handle across a pathname replacement.
pub struct InspectedRecord(RecordFile);

impl std::fmt::Debug for InspectedRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .finish_non_exhaustive()
    }
}

impl InspectedRecord {
    /// Captures the file currently addressed by a pathname.
    pub fn open(path: &Path) -> io::Result<Self> {
        RecordFile::open(path).map(Self)
    }

    /// Deletes the captured file, not a successor at the same pathname.
    pub fn delete(&self) -> io::Result<()> {
        self.0.delete()
    }
}

/// Exercises failed-install cleanup without exposing PAL error types.
pub fn discard_failed_claim(path: &Path) {
    session_store::remove_failed_staging(path, &Err(PalError::new(PalErrorKind::Other)));
}

/// Reads bytes while treating an absent file as an absent record.
pub fn read_if_present(path: &Path) -> io::Result<Option<Vec<u8>>> {
    session_store::read_if_present(path)
}

/// Reads the owner from a claim's production encoding.
pub fn reservation_owner(bytes: &[u8]) -> io::Result<Identity> {
    match serde_json::from_slice(bytes).map_err(io::Error::other)? {
        StoredSession::Reserved { owner } => Ok(Identity(owner)),
        StoredSession::Published(_) => Err(io::Error::other("expected a reservation")),
    }
}

/// Resolves the user's default store without creating it.
pub fn default_store_root() -> io::Result<PathBuf> {
    resolve_store_root(None).map_err(io::Error::other)
}
