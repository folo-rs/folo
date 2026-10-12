//! Filesystem session store.
//!
//! Id allocation uses exclusive file creation so two concurrent `run`
//! invocations cannot take the same id.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::SessionId;
use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::session_store::SessionStore;
use crate::pal::session_store::fs_store::windows::{
    RecordFile, move_file_no_replace, move_file_replace,
};
use crate::pal::session_store::stored::StoredSession;
use crate::session_record::{ProcessIdentity, SessionRecord};

/// Session store rooted at a caller-supplied directory.
#[derive(Clone, Debug)]
pub(crate) struct FsSessionStore {
    root: PathBuf,
}

fn parse_stored(bytes: &[u8]) -> Result<StoredSession, PalError> {
    serde_json::from_slice(bytes).map_err(|error| {
        PalError::with_source(
            PalErrorKind::Other,
            io::Error::new(io::ErrorKind::InvalidData, error),
        )
    })
}

fn parse_record(bytes: &[u8], expected: SessionId) -> Result<Option<SessionRecord>, PalError> {
    let StoredSession::Published(record) = parse_stored(bytes)? else {
        return Ok(None);
    };
    if record.id != expected {
        return Ok(None);
    }
    Ok(Some(record))
}

fn replace_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    move_file_replace(tmp, dest)
}

/// Converts the impossible serialization failure for a fixed stored-record
/// shape into the store's error type.
///
/// A failure here requires a defect in `serde_json`, so no input can exercise it.
#[cfg_attr(coverage_nightly, coverage(off))]
fn serialization_error(error: serde_json::Error) -> PalError {
    PalError::with_source(
        PalErrorKind::Other,
        io::Error::new(io::ErrorKind::InvalidData, error),
    )
}

/// Advances an occupied session id, or reports that the complete id space is used.
fn next_session_id(id: SessionId) -> Result<SessionId, PalError> {
    id.get()
        .checked_add(1)
        .and_then(SessionId::from_u32)
        .ok_or_else(|| PalError::new(PalErrorKind::Other))
}

/// Removes a staged claim after an install failure.
pub(crate) fn remove_failed_staging(staging: &Path, installed: &Result<SessionId, PalError>) {
    if installed.is_err() {
        // A staging file nobody installs is this call's to remove; leaving it
        // would litter the store with files no reader understands.
        _ = fs::remove_file(staging);
    }
}

impl FsSessionStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn record_path(&self, id: SessionId) -> PathBuf {
        self.root.join(format!("{}.json", id.get()))
    }

    /// Where a claim is assembled before it is given an id's name.
    ///
    /// The name carries the process identity and a counter unique within that
    /// process, so no two allocations share a staging file and a reused pid
    /// cannot collide with a file left by its predecessor. The extension is one
    /// no reader looks at, so a half-written claim is never mistaken for a session.
    fn staging_path(&self, owner: &ProcessIdentity) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let attempt = NEXT.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!(
            "{}-{}-{attempt}.claim",
            owner.pid, owner.creation_time
        ))
    }

    /// Gives the staged claim the name of the smallest id it can take.
    ///
    /// Installing fails rather than replaces when the name is taken, so an id
    /// another process claimed first is simply the next one tried.
    // A mutation of the taken-vs-failed guard turns the search for a free id
    // into one that walks the whole `u32` range against a store that cannot
    // accept any of them, which no test can outlast and watchdogs are disabled
    // under cargo-mutants. Ref: docs/testing.md, "Mutation testing".
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn install_claim(&self, staging: &Path) -> Result<SessionId, PalError> {
        let mut id = SessionId::MIN;
        loop {
            match move_file_no_replace(staging, &self.record_path(id)) {
                Ok(()) => return Ok(id),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    id = next_session_id(id)?;
                }
                Err(error) => return Err(PalError::from_io(error)),
            }
        }
    }

    /// Every readable record file, paired with the id its name encodes.
    ///
    /// Foreign, torn, and unparseable files are skipped: one bad file must not
    /// hide every session in the store from `dure list`.
    ///
    /// A file that names a session but cannot be read is different: it is a
    /// session the store knows about and cannot report. That is an error, so a
    /// permission or sharing problem shows up as one rather than as an empty
    /// session list. Ref: docs/session-store.md.
    fn stored(&self) -> Result<Vec<(SessionId, StoredSession)>, PalError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(PalError::from_io(error)),
        };
        let mut stored = Vec::new();
        for entry in entries {
            let entry = entry.map_err(PalError::from_io)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            let Ok(raw) = stem.parse::<u32>() else {
                continue;
            };
            let Some(id) = SessionId::from_u32(raw) else {
                continue;
            };
            let bytes = match read_if_present(&entry.path()) {
                Ok(Some(bytes)) => bytes,
                // A file that disappeared between the listing and the read is
                // a session that ended, which is the store working.
                Ok(None) => continue,
                Err(error) => return Err(PalError::from_io(error)),
            };
            let Ok(parsed) = parse_stored(&bytes) else {
                continue;
            };
            stored.push((id, parsed));
        }
        stored.sort_by_key(|(id, _stored)| id.get());
        Ok(stored)
    }
}

impl SessionStore for FsSessionStore {
    #[cfg(test)]
    fn root(&self) -> PathBuf {
        self.root.clone()
    }

    fn allocate_id(&self, owner: &ProcessIdentity) -> Result<SessionId, PalError> {
        fs::create_dir_all(&self.root).map_err(PalError::from_io)?;
        let claim = serde_json::to_vec(&StoredSession::Reserved { owner: *owner })
            .map_err(serialization_error)?;
        // Built under a name nobody looks for, so the claim is complete on disk
        // before it is given the name that means "this id is taken". A claim
        // that named nobody could never be proved abandoned, and its id would
        // stay occupied for the rest of the logon session.
        // Ref: docs/session-store.md, "Claimed and published".
        let staging = self.staging_path(owner);
        write_new_file(&staging, &claim).map_err(PalError::from_io)?;
        let installed = self.install_claim(&staging);
        remove_failed_staging(&staging, &installed);
        installed
    }

    fn publish(&self, record: &SessionRecord) -> Result<(), PalError> {
        fs::create_dir_all(&self.root).map_err(PalError::from_io)?;
        let id = record.id;
        let path = self.record_path(id);
        let tmp = self.root.join(format!("{}.json.tmp", id.get()));
        let json = serde_json::to_vec_pretty(&StoredSession::Published(record.clone()))
            .map_err(serialization_error)?;
        let mut file = File::create(&tmp).map_err(PalError::from_io)?;
        file.write_all(&json).map_err(PalError::from_io)?;
        file.sync_all().map_err(PalError::from_io)?;
        drop(file);
        replace_file(&tmp, &path).map_err(PalError::from_io)
    }

    fn read(&self, id: SessionId) -> Result<Option<SessionRecord>, PalError> {
        let path = self.record_path(id);
        match fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) => parse_record(&bytes, id),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(PalError::from_io(error)),
        }
    }

    fn list(&self) -> Result<Vec<SessionRecord>, PalError> {
        Ok(self
            .stored()?
            .into_iter()
            .filter_map(|(id, stored)| match stored {
                StoredSession::Published(record) if record.id == id => Some(record),
                _ => None,
            })
            .collect())
    }

    fn list_reservations(&self) -> Result<Vec<(SessionId, ProcessIdentity)>, PalError> {
        Ok(self
            .stored()?
            .into_iter()
            .filter_map(|(id, stored)| match stored {
                StoredSession::Reserved { owner } => Some((id, owner)),
                StoredSession::Published(_record) => None,
            })
            .collect())
    }

    fn delete_owned_by(&self, id: SessionId, owner: &ProcessIdentity) -> Result<(), PalError> {
        // Reading and deleting through one handle is what makes this safe to race: ids are
        // reused, so between deciding and deleting the name can come to mean a live session
        // somebody else just published. The handle addresses the file the decision was made
        // about, so such a replacement is never the file removed.
        let file = match RecordFile::open(&self.record_path(id)) {
            Ok(file) => file,
            Err(error) if is_absent(&error) => return Ok(()),
            Err(error) => return Err(PalError::from_io(error)),
        };
        let bytes = file.read().map_err(PalError::from_io)?;
        // A file that no longer names `owner` belongs to whoever claimed the id
        // after the caller read it, and a file nothing can parse names nobody.
        let owned = match parse_stored(&bytes) {
            Ok(StoredSession::Reserved { owner: current }) => current == *owner,
            Ok(StoredSession::Published(record)) => record.supervisor == *owner,
            Err(_error) => false,
        };
        if !owned {
            return Ok(());
        }
        file.delete().map_err(PalError::from_io)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PalError> {
        fs::canonicalize(path).map_err(PalError::from_io)
    }

    fn current_dir(&self) -> Result<PathBuf, PalError> {
        env::current_dir().map_err(PalError::from_io)
    }
}

/// Whether a failure to open a record means the name simply is not there.
///
/// Nothing to delete is not a failure to delete, so an id whose file is already gone is a
/// caller asking for a state that already holds. Any other open failure is a real fault.
fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound)
}

/// The file's bytes, or nothing if it is already gone.
///
/// A record can be deleted between a directory listing and the read that
/// follows it, which is a session ending rather than a store this process
/// cannot read.
pub(crate) fn read_if_present(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Writes `content` to a file that must not already exist, and flushes it.
///
/// Flushed before returning so that whatever is given this file's name next is
/// backed by content that has reached the disk.
fn write_new_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn advancing_the_last_session_id_reports_exhaustion() {
        let last = SessionId::from_u32(u32::MAX).unwrap();
        next_session_id(last).unwrap_err();
    }

    #[test]
    fn a_staging_name_carries_the_complete_process_identity() {
        let store = FsSessionStore::new(PathBuf::from("store"));
        let owner = ProcessIdentity {
            pid: 73_u32.try_into().unwrap(),
            creation_time: 987_654_321,
        };
        let name = store
            .staging_path(&owner)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        assert!(name.starts_with(&format!("{}-{}-", owner.pid, owner.creation_time)));
    }
}
