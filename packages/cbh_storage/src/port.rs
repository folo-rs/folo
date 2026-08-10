use std::fmt;
use std::future::Future;

use crate::StorageError;

/// An object store of immutable, key-addressed result sets.
///
/// The model — flat string keys, write-once objects, and list-by-prefix — is the
/// lowest common denominator of a filesystem and a blob container, so every
/// backend implements this trait with no special-casing by callers.
pub trait Storage: fmt::Debug + Send + Sync {
    /// Writes `bytes` at `key`, creating any intermediate structure as needed.
    ///
    /// Storage is write-once: an existing object is never overwritten.
    ///
    /// # Errors
    ///
    /// Returns a [`StorageError`] if the key is malformed, the object cannot be
    /// written, or an object is already stored at `key`. In the last case,
    /// [`StorageError::already_existing_key`] returns `Some(key)`.
    fn put(&self, key: &str, bytes: &[u8]) -> impl Future<Output = Result<(), StorageError>>;

    /// Writes `bytes` at `key`, replacing any object already stored there.
    ///
    /// Unlike [`put`](Self::put), this never fails because an object already
    /// exists; it is the explicit escape hatch from the write-once contract that
    /// `collect --overwrite` and `backfill --overwrite` use to regenerate a data
    /// point. Intermediate structure is created as needed.
    ///
    /// The returned future is `Send` for the same reason [`get`](Self::get)'s is:
    /// the read-through cache populates its mirror with this method *inside* a
    /// (spawnable) `get`, so the populate future must be sendable across worker
    /// threads too.
    ///
    /// # Errors
    ///
    /// Returns a [`StorageError`] if the key is malformed or the object cannot be
    /// written.
    fn put_overwrite(
        &self,
        key: &str,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), StorageError>> + Send;

    /// Reads the object stored at `key`.
    ///
    /// The returned future is `Send` so loads can run on spawned worker tasks
    /// (the analyze pipeline fans object decompress + parse out across cores).
    ///
    /// # Errors
    ///
    /// Returns a [`StorageError`] if the key is malformed or the object cannot be
    /// read. [`StorageError::is_not_found`] returns `true` when no object exists at
    /// `key`.
    fn get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StorageError>> + Send;

    /// Lists the keys of all objects whose key starts with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns a [`StorageError`] if the listing cannot be produced.
    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<String>, StorageError>>;

    /// Removes the object stored at `key`.
    ///
    /// # Errors
    ///
    /// Returns a [`StorageError`] if the key is malformed or the object cannot be
    /// removed. [`StorageError::is_not_found`] returns `true` when no object exists
    /// at `key`.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StorageError>>;
}
