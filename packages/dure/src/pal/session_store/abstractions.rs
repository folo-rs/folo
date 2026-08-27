//! Session store PAL: records, id allocation, and path canonicalization.

use std::path::{Path, PathBuf};

use crate::pal::error::PalError;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;

/// Per-user filesystem of live-session records.
///
/// The real implementation uses per-user `LocalAppData`, subdirectory `dure`.
/// Tests supply an isolated root instead of the user's store.
/// Ref: docs/implementation.md, PAL slicing and "Session store".
#[cfg_attr(test, mockall::automock)]
pub(crate) trait SessionStore: Send + Sync + std::fmt::Debug + 'static {
    /// Directory that holds session record files.
    #[cfg(test)]
    fn root(&self) -> PathBuf;

    /// Smallest unused positive integer, filesystem-coordinated.
    fn allocate_id(&self) -> Result<SessionId, PalError>;

    /// Overwrites the record file for this id.
    fn publish(&self, record: &SessionRecord) -> Result<(), PalError>;

    /// Reads one record, or `None` if the file is absent.
    fn read(&self, id: SessionId) -> Result<Option<SessionRecord>, PalError>;

    /// Every parseable record in the store, including stale ones.
    fn list(&self) -> Result<Vec<SessionRecord>, PalError>;

    /// Removes the record file. Missing files succeed.
    fn delete(&self, id: SessionId) -> Result<(), PalError>;

    /// Canonical absolute form used as the launch-directory key.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PalError>;

    /// Current working directory of this process.
    fn current_dir(&self) -> Result<PathBuf, PalError>;
}
