//! The storage abstraction: an immutable, list-by-prefix object store that both a
//! local filesystem and an Azure Blob container implement identically.

mod azure;
mod caching;
mod facade;
mod github_oidc;
mod local;

use std::error::Error;
use std::future::Future;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fmt, io};

use cargo_bench_history_core::model::{OBJECTS_SEGMENT, STORAGE_VERSION, sanitize_segment};
pub(crate) use facade::{StorageFacade, build_storage, resolve_storage};
pub use facade::{StorageOverride, azure_backend_from_parts};

/// An object store of immutable, key-addressed result sets.
///
/// The model — flat string keys, write-once objects, and list-by-prefix — is the
/// lowest common denominator of a filesystem and a blob container, so every
/// backend implements this trait with no special-casing by callers.
pub(crate) trait Storage: fmt::Debug + Send + Sync {
    /// Writes `bytes` at `key`, creating any intermediate structure as needed.
    ///
    /// Storage is write-once: an existing object is never overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidKey`] if `key` is malformed,
    /// [`StorageError::AlreadyExists`] if an object is already stored at `key`,
    /// or [`StorageError::Io`] if the object cannot be written.
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
    /// Returns [`StorageError::InvalidKey`] if `key` is malformed, or
    /// [`StorageError::Io`] if the object cannot be written.
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
    /// Returns [`StorageError::InvalidKey`] if `key` is malformed,
    /// [`StorageError::NotFound`] if no object exists at `key`, or
    /// [`StorageError::Io`] if it cannot be read.
    fn get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StorageError>> + Send;

    /// Lists the keys of all objects whose key starts with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Io`] if the listing cannot be produced.
    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<String>, StorageError>>;

    /// Removes the object stored at `key`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidKey`] if `key` is malformed,
    /// [`StorageError::NotFound`] if no object exists at `key`, or
    /// [`StorageError::Io`] if it cannot be removed.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StorageError>>;
}

/// An error from a storage operation.
#[derive(Debug)]
pub enum StorageError {
    /// No object exists at the requested key.
    NotFound {
        /// The key that was not found.
        key: String,
    },
    /// The key was not a valid storage key (it contained an empty, `.`, or `..`
    /// segment, or a platform-absolute segment, that could escape the storage
    /// root).
    InvalidKey {
        /// The rejected key.
        key: String,
    },
    /// An object already exists at the requested key. Storage is write-once, so
    /// an existing object is never overwritten.
    AlreadyExists {
        /// The key that was already occupied.
        key: String,
    },
    /// The storage backend is misconfigured (e.g. an Azure endpoint that is not a
    /// valid HTTPS URL).
    Config {
        /// Human-readable description of the misconfiguration.
        message: String,
    },
    /// An underlying I/O error occurred.
    Io(io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { key } => write!(f, "object not found: {key}"),
            Self::InvalidKey { key } => write!(f, "invalid storage key: {key}"),
            Self::AlreadyExists { key } => write!(f, "object already exists: {key}"),
            Self::Config { message } => write!(f, "storage configuration error: {message}"),
            Self::Io(error) => write!(f, "storage I/O error: {error}"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NotFound { .. }
            | Self::InvalidKey { .. }
            | Self::AlreadyExists { .. }
            | Self::Config { .. } => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// Returns `true` only if `segment` is a single ordinary path component, so it
/// can never escape or rebind a storage root when appended to a key.
pub(crate) fn is_plain_segment(segment: &str) -> bool {
    let mut components = Path::new(segment).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Validates that `key` is a well-formed object key: a `/`-separated sequence of
/// ordinary path segments. Rejects empty, `.`, `..`, and platform-absolute
/// segments, which could otherwise escape or rebind a filesystem storage root.
///
/// Both backends share this so the in-memory fake rejects exactly the keys the
/// real [`LocalStorage`](local::LocalStorage) would, keeping fake-driven tests
/// honest.
///
/// # Errors
///
/// Returns [`StorageError::InvalidKey`] if any segment of `key` is not a single
/// ordinary path component.
pub(crate) fn validate_key(key: &str) -> Result<(), StorageError> {
    for segment in key.split('/') {
        if !is_plain_segment(segment) {
            return Err(StorageError::InvalidKey {
                key: key.to_owned(),
            });
        }
    }
    Ok(())
}

/// The reserved file-name segment of the per-project cache-invalidation marker.
///
/// It is a **metadata sibling** of the project's data subtree: data objects live
/// under `{STORAGE_VERSION}/<project>/{OBJECTS_SEGMENT}/…`, while the marker sits
/// directly under `{STORAGE_VERSION}/<project>/`, so it can never appear in a data
/// listing (which narrows to the `objects/` prefix) and can never be mistaken for a
/// data key. The leading `_` keeps it visually distinct from future metadata kinds.
const CACHE_EPOCH_SEGMENT: &str = "_cache-epoch";

/// The storage key of a project's cache-invalidation marker:
/// `{STORAGE_VERSION}/<project>/_cache-epoch`.
///
/// A cloud writer overwrites it with a fresh opaque epoch token whenever it
/// removes or rewrites an existing object (`delete`/`put_overwrite`); the
/// read-through cache compares its cloud value against the copy its mirror recorded
/// under the *same key* to decide whether to reuse or wipe that project's mirrored
/// objects. The `project` segment is sanitized identically to
/// [`DiscriminantSet::partition_prefix`](cargo_bench_history_core::model::DiscriminantSet::partition_prefix)
/// so the marker lands in the same partition as that project's data.
pub(crate) fn cache_epoch_key(project: &str) -> String {
    format!(
        "{STORAGE_VERSION}/{project}/{CACHE_EPOCH_SEGMENT}",
        project = sanitize_segment(project)
    )
}

/// The listing prefix that covers every **data** object of `project`:
/// `{STORAGE_VERSION}/<project>/{OBJECTS_SEGMENT}/`.
///
/// Data loaders (`analyze`/`list`/`prune`, `backfill`'s recorded-commit scan) list
/// under this prefix so they enumerate only benchmark objects, never a per-project
/// metadata sibling such as the [`cache_epoch_key`] marker. The read-through cache
/// also wipes exactly this subtree when invalidating one project's mirror.
pub(crate) fn project_objects_prefix(project: &str) -> String {
    format!(
        "{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/",
        project = sanitize_segment(project)
    )
}

/// A one-shot "a cloud object was removed or overwritten" flag.
///
/// A cloud writer [`arm`](Self::arm)s it on a `delete` or on a `put_overwrite`
/// that *replaces an existing object* — the only operations that break the storage
/// model's per-key immutability — and a single flush at the end of a command
/// [`take`](Self::take)s it and, when set, writes one fresh [`cache_epoch_key`]
/// marker. Coalescing every mutation in a command into a single marker write keeps
/// the bump to one round-trip. Creating a new key never arms it — whether through
/// write-once `put` (the additive path the CI collection uses) or a
/// `put_overwrite` that turns out to add rather than replace — so an append-only
/// run never invalidates the cache, and a read-through mirror still discovers the
/// new key through its always-fresh listing.
#[derive(Debug, Default)]
pub(crate) struct PendingInvalidation {
    armed: AtomicBool,
}

impl PendingInvalidation {
    /// Arms the flag so a later [`take`](Self::take) reports a pending bump.
    pub(crate) fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    /// Returns whether the flag was armed and clears it, so a second flush in the
    /// same process performs no further marker write.
    pub(crate) fn take(&self) -> bool {
        self.armed.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn not_found_display_includes_key() {
        let error = StorageError::NotFound {
            key: "v1/x".to_owned(),
        };
        assert_eq!(error.to_string(), "object not found: v1/x");
    }

    #[test]
    fn io_display_and_source() {
        let error = StorageError::Io(io::Error::other("disk gone"));
        assert!(error.to_string().contains("disk gone"));
        assert!(error.source().is_some());
    }

    #[test]
    fn not_found_has_no_source() {
        let error = StorageError::NotFound {
            key: "k".to_owned(),
        };
        assert!(error.source().is_none());
    }

    #[test]
    fn invalid_key_display_and_no_source() {
        let error = StorageError::InvalidKey {
            key: "v1/../escape".to_owned(),
        };
        assert!(error.to_string().contains("v1/../escape"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn already_exists_display_and_no_source() {
        let error = StorageError::AlreadyExists {
            key: "v1/dup".to_owned(),
        };
        assert!(error.to_string().contains("v1/dup"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn config_display_and_no_source() {
        let error = StorageError::Config {
            message: "both keys set".to_owned(),
        };
        assert!(error.to_string().contains("both keys set"), "{error}");
        assert!(error.to_string().contains("configuration"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn cache_epoch_key_lands_in_the_project_partition() {
        // The marker is a metadata sibling of the project's objects/ subtree: it sits
        // directly under v1/<project>/, so it never appears in a data listing (which
        // narrows to the objects/ prefix) and cannot collide with a data object.
        assert_eq!(cache_epoch_key("folo"), "v1/folo/_cache-epoch");
        // The project is sanitized identically to the data partition, so the
        // marker always lands beside that project's objects.
        assert_eq!(cache_epoch_key("a b"), "v1/a_b/_cache-epoch");
    }

    #[test]
    fn project_objects_prefix_narrows_to_the_data_subtree() {
        // Data loaders list this prefix so they enumerate only benchmark objects and
        // never the sibling cache-epoch marker; the project is sanitized identically.
        assert_eq!(project_objects_prefix("folo"), "v1/folo/objects/");
        assert_eq!(project_objects_prefix("a b"), "v1/a_b/objects/");
        // The marker sits directly under the project partition, outside this prefix,
        // so a data listing can never pick it up.
        assert!(!cache_epoch_key("folo").starts_with(&project_objects_prefix("folo")));
    }

    #[test]
    fn pending_invalidation_starts_unarmed() {
        let pending = PendingInvalidation::default();
        assert!(!pending.take(), "a fresh flag is not armed");
    }

    #[test]
    fn pending_invalidation_arms_and_takes_once() {
        let pending = PendingInvalidation::default();
        pending.arm();
        assert!(pending.take(), "an armed flag reports a pending bump");
        assert!(
            !pending.take(),
            "taking clears the flag so a second flush is a no-op"
        );
    }

    #[test]
    fn pending_invalidation_arming_is_idempotent() {
        let pending = PendingInvalidation::default();
        pending.arm();
        pending.arm();
        assert!(
            pending.take(),
            "repeated arming still reports one pending bump"
        );
        assert!(!pending.take());
    }

    #[test]
    fn memory_storage_put_get_keys_and_list() {
        let storage = MemoryStorage::new();
        block_on(storage.put("v1/a/1.json", b"1")).unwrap();
        block_on(storage.put("v1/a/2.json", b"2")).unwrap();
        block_on(storage.put("v1/b/3.json", b"3")).unwrap();

        assert_eq!(block_on(storage.get("v1/a/1.json")).unwrap(), b"1");
        assert_eq!(
            storage.keys(),
            vec![
                "v1/a/1.json".to_owned(),
                "v1/a/2.json".to_owned(),
                "v1/b/3.json".to_owned(),
            ]
        );
        assert_eq!(
            block_on(storage.list("v1/a/")).unwrap(),
            vec!["v1/a/1.json".to_owned(), "v1/a/2.json".to_owned()]
        );
    }

    #[test]
    fn memory_storage_get_missing_is_not_found() {
        let storage = MemoryStorage::new();
        let error = block_on(storage.get("absent")).unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }));
    }

    #[test]
    fn memory_storage_rejects_duplicate_key() {
        let storage = MemoryStorage::new();
        block_on(storage.put("dup", b"1")).unwrap();
        let error = block_on(storage.put("dup", b"2")).unwrap_err();
        assert!(
            matches!(error, StorageError::AlreadyExists { .. }),
            "{error:?}"
        );
        // The original value is preserved (write-once).
        assert_eq!(block_on(storage.get("dup")).unwrap(), b"1");
    }

    #[test]
    fn memory_storage_put_overwrite_replaces_an_existing_object() {
        let storage = MemoryStorage::new();
        block_on(storage.put("k", b"first")).unwrap();
        block_on(storage.put_overwrite("k", b"second")).unwrap();
        assert_eq!(block_on(storage.get("k")).unwrap(), b"second");
    }

    #[test]
    fn memory_storage_put_overwrite_creates_when_absent() {
        let storage = MemoryStorage::new();
        block_on(storage.put_overwrite("k", b"only")).unwrap();
        assert_eq!(block_on(storage.get("k")).unwrap(), b"only");
    }

    #[test]
    fn memory_storage_put_overwrite_rejects_malformed_keys() {
        let storage = MemoryStorage::new();
        let error = block_on(storage.put_overwrite("v1/../escape", b"x")).unwrap_err();
        assert!(
            matches!(error, StorageError::InvalidKey { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn memory_storage_delete_removes_an_object() {
        let storage = MemoryStorage::new();
        block_on(storage.put("v1/a/1.json", b"1")).unwrap();
        block_on(storage.put("v1/a/2.json", b"2")).unwrap();

        block_on(storage.delete("v1/a/1.json")).unwrap();

        // Only the targeted key is gone; the sibling object is untouched.
        assert_eq!(storage.keys(), vec!["v1/a/2.json".to_owned()]);
        let error = block_on(storage.get("v1/a/1.json")).unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn memory_storage_delete_missing_key_is_not_found() {
        let storage = MemoryStorage::new();
        let error = block_on(storage.delete("v1/absent.json")).unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn memory_storage_delete_rejects_malformed_keys() {
        let storage = MemoryStorage::new();
        let error = block_on(storage.delete("v1/../escape")).unwrap_err();
        assert!(
            matches!(error, StorageError::InvalidKey { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn memory_storage_rejects_the_same_malformed_keys_as_local_storage() {
        let storage = MemoryStorage::new();
        // Empty, `.`, `..`, and absolute segments could escape a filesystem root,
        // so the fake must reject them exactly as `LocalStorage` does.
        for bad in ["v1/../escape", "v1//gap", "v1/./here", "", "/v1/abs"] {
            let put = block_on(storage.put(bad, b"x")).unwrap_err();
            assert!(
                matches!(put, StorageError::InvalidKey { .. }),
                "put {bad:?}: {put:?}"
            );
            let get = block_on(storage.get(bad)).unwrap_err();
            assert!(
                matches!(get, StorageError::InvalidKey { .. }),
                "get {bad:?}: {get:?}"
            );
        }
    }
}

/// An in-memory [`Storage`] for tests: write-once keys held in a sorted map.
///
/// Its async methods complete synchronously (no real I/O), so orchestration tests
/// can drive them with a reactor-free `block_on` and remain Miri-safe.
///
/// Unlike the real backends, this fake stores object bodies **uncompressed**. Its
/// contract is the key/value model — `put X` then `get X` returns `X`, with the
/// same key validation — which holds identically whether or not bodies are gzip,
/// and no test inspects a raw stored body (the fake exposes only [`keys`](Self::keys)).
/// Skipping the codec keeps the Miri-driven orchestration suite fast and free of
/// gzip on its hot path; the codec itself is exercised by its own unit tests and
/// by the real backends.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryStorage {
    objects: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>,
}

#[cfg(test)]
impl MemoryStorage {
    /// Creates an empty in-memory store.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns the stored keys in sorted order.
    pub(crate) fn keys(&self) -> Vec<String> {
        self.objects.lock().unwrap().keys().cloned().collect()
    }
}

#[cfg(test)]
impl Storage for MemoryStorage {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        validate_key(key)?;
        let mut objects = self.objects.lock().unwrap();
        if objects.contains_key(key) {
            return Err(StorageError::AlreadyExists {
                key: key.to_owned(),
            });
        }
        objects.insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        validate_key(key)?;
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        validate_key(key)?;
        self.objects
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| StorageError::NotFound {
                key: key.to_owned(),
            })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        validate_key(key)?;
        self.objects
            .lock()
            .unwrap()
            .remove(key)
            .map(|_| ())
            .ok_or_else(|| StorageError::NotFound {
                key: key.to_owned(),
            })
    }
}
