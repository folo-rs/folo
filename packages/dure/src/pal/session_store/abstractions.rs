//! Session store PAL: records, id allocation, and path canonicalization.

use std::path::{Path, PathBuf};

use crate::pal::error::PalError;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;

/// Create, read, list, and delete session records, and allocate ids.
///
/// The real implementation uses per-user `LocalAppData`, subdirectory `dure`.
/// The root is supplied so tests never touch the user's real store.
/// Ref: docs/implementation.md, PAL slicing and "Session store".
#[cfg_attr(test, mockall::automock)]
pub(crate) trait SessionStore: Send + Sync + std::fmt::Debug + 'static {
    /// Directory that holds session record files.
    #[allow(
        dead_code,
        reason = "PAL surface for tests; production never reads the root back"
    )]
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
