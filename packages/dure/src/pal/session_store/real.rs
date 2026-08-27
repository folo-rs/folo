//! Filesystem session store.
//!
//! Id allocation uses exclusive file creation so two concurrent `run`
//! invocations cannot take the same id.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::session_store::SessionStore;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;

/// Session store rooted at a caller-supplied directory.
#[derive(Clone, Debug)]
pub(crate) struct FsSessionStore {
    root: PathBuf,
}

fn parse_record(bytes: &[u8]) -> Result<Option<SessionRecord>, PalError> {
    let record: SessionRecord = serde_json::from_slice(bytes).map_err(|error| {
        PalError::with_source(
            PalErrorKind::Other,
            io::Error::new(io::ErrorKind::InvalidData, error),
        )
    })?;
    if SessionId::from_u32(record.id).is_none() {
        return Err(PalError::new(PalErrorKind::Other));
    }
    Ok(Some(record))
}

impl FsSessionStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn record_path(&self, id: SessionId) -> PathBuf {
        self.root.join(format!("{}.json", id.get()))
    }
}

impl SessionStore for FsSessionStore {
    fn root(&self) -> PathBuf {
        self.root.clone()
    }

    fn allocate_id(&self) -> Result<SessionId, PalError> {
        fs::create_dir_all(&self.root).map_err(PalError::from_io)?;
        let mut n: u32 = 1;
        loop {
            let Some(id) = SessionId::from_u32(n) else {
                return Err(PalError::new(PalErrorKind::Other));
            };
            let path = self.record_path(id);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(b"").map_err(PalError::from_io)?;
                    return Ok(id);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    n = n
                        .checked_add(1)
                        .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
                }
                Err(error) => return Err(PalError::from_io(error)),
            }
        }
    }

    fn publish(&self, record: &SessionRecord) -> Result<(), PalError> {
        fs::create_dir_all(&self.root).map_err(PalError::from_io)?;
        let id = record.session_id();
        let path = self.record_path(id);
        let json = serde_json::to_vec_pretty(record).map_err(|error| {
            PalError::with_source(
                PalErrorKind::Other,
                io::Error::new(io::ErrorKind::InvalidData, error),
            )
        })?;
        fs::write(path, json).map_err(PalError::from_io)
    }

    fn read(&self, id: SessionId) -> Result<Option<SessionRecord>, PalError> {
        let path = self.record_path(id);
        match fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) => parse_record(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(PalError::from_io(error)),
        }
    }

    fn list(&self) -> Result<Vec<SessionRecord>, PalError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(PalError::from_io(error)),
        };
        let mut records = Vec::new();
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
            if let Ok(Some(record)) = self.read(id) {
                records.push(record);
            }
        }
        records.sort_by_key(|record| record.id);
        Ok(records)
    }

    fn delete(&self, id: SessionId) -> Result<(), PalError> {
        match fs::remove_file(self.record_path(id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PalError::from_io(error)),
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PalError> {
        fs::canonicalize(path).map_err(PalError::from_io)
    }

    fn current_dir(&self) -> Result<PathBuf, PalError> {
        std::env::current_dir().map_err(PalError::from_io)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::iter;
    use std::path::Path;
    use std::thread;

    use tempfile::TempDir;
    use testing::with_watchdog;

    use super::*;

    fn store() -> (TempDir, FsSessionStore) {
        let dir = TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    fn record(id: SessionId, dir: &Path) -> SessionRecord {
        SessionRecord {
            id: id.get(),
            supervisor_pid: 1,
            supervisor_creation_time: 1,
            pipe_name: "pipe".to_string(),
            launch_directory: dir.to_path_buf(),
            command: vec!["app.exe".to_string()],
            started_at_unix_ms: 1,
            attached: false,
        }
    }

    #[test]
    fn allocates_smallest_unused_and_reuses_after_delete() {
        let (dir, store) = store();
        assert_eq!(store.root(), dir.path());
        let first = store.allocate_id().unwrap();
        let second = store.allocate_id().unwrap();
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        store.delete(first).unwrap();
        let reused = store.allocate_id().unwrap();
        assert_eq!(reused.get(), 1);
    }

    #[test]
    fn concurrent_allocations_are_unique() {
        with_watchdog(|| {
            let dir = TempDir::new().unwrap();
            let root = dir.path().to_path_buf();
            let threads: Vec<_> = iter::repeat_with(|| {
                let root = root.clone();
                thread::spawn(move || {
                    let store = FsSessionStore::new(root);
                    store.allocate_id().unwrap()
                })
            })
            .take(8)
            .collect();
            let mut ids = HashSet::new();
            for handle in threads {
                assert!(ids.insert(handle.join().unwrap().get()));
            }
            assert_eq!(ids.len(), 8);
        });
    }

    #[test]
    fn publish_read_list_delete() {
        let (dir, store) = store();
        let id = store.allocate_id().unwrap();
        let rec = record(id, dir.path());
        store.publish(&rec).unwrap();
        assert_eq!(store.read(id).unwrap().unwrap(), rec);
        assert_eq!(store.list().unwrap(), vec![rec]);
        store.delete(id).unwrap();
        assert!(store.read(id).unwrap().is_none());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_skips_corrupt_and_zero_id_records() {
        let (dir, store) = store();
        fs::write(dir.path().join("0.json"), b"{\"id\":0}").unwrap();
        fs::write(dir.path().join("not-json.json"), b"nope").unwrap();
        let id = store.allocate_id().unwrap();
        let rec = record(id, dir.path());
        store.publish(&rec).unwrap();
        assert_eq!(store.list().unwrap(), vec![rec]);
    }
}
