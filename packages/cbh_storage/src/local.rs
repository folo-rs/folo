//! [`LocalStorage`]: a [`Storage`] backed by a directory tree on the local
//! filesystem. Object keys map to relative paths under a configured root.

use std::ffi::OsStr;
#[cfg(unix)]
use std::fs::Permissions;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::Builder;

use crate::{
    AdvanceLocalListingDirectoryError, CreateLocalParentDirectoriesError,
    DecompressLocalObjectError, InspectLocalListingEntryError, ObjectAlreadyExistsError,
    ObjectNotFoundError, OpenLocalListingDirectoryError, ReadLocalObjectError,
    RemoveLocalObjectError, Storage, StorageError, WriteLocalObjectError, is_plain_segment,
    validate_key,
};

/// Filename prefix for the transient files an atomic write publishes.
/// The prefix is reserved: real object keys are `.json` files named from commit
/// hashes and timestamps, so nothing stored ever starts with it, which lets
/// [`list`](LocalStorage::list) skip a crash-orphaned temp file rather than
/// mistake it for an object.
const TEMP_PREFIX: &str = ".cbh-tmp-";

/// Matches ordinary file creation; the process umask determines the final access bits.
#[cfg(unix)]
const CREATION_PERMISSIONS: u32 = 0o666;

/// A [`Storage`] that persists objects as files under a root directory.
#[derive(Clone, Debug)]
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    /// Creates a local storage rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Maps an object key to a path under the root, rejecting any segment that
    /// is not a single ordinary path component. This excludes empty, `.`, and
    /// `..` segments as well as platform-absolute segments (e.g. a Windows
    /// `C:\\...` or UNC `\\\\server\\share` segment, which `PathBuf::push` would
    /// otherwise treat as absolute and use to discard the configured root).
    fn key_path(&self, key: &str) -> Result<PathBuf, StorageError> {
        validate_key(key)?;
        let mut path = self.root.clone();
        for segment in key.split('/') {
            path.push(segment);
        }
        Ok(path)
    }
}

impl Storage for LocalStorage {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.key_path(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                CreateLocalParentDirectoriesError::caused_by(parent.to_path_buf(), error)
            })?;
        }
        let compressed = cbh_codec::compress(bytes);
        write_atomic(key, path, compressed, false).await
    }

    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.key_path(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                CreateLocalParentDirectoriesError::caused_by(parent.to_path_buf(), error)
            })?;
        }
        let compressed = cbh_codec::compress(bytes);
        write_atomic(key, path, compressed, true).await
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let path = self.key_path(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => cbh_codec::decompress(&bytes)
                .map_err(|error| DecompressLocalObjectError::caused_by(path, error).into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(ObjectNotFoundError::caused_by(key.to_owned(), error).into())
            }
            Err(error) => Err(ReadLocalObjectError::caused_by(path, error).into()),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        // Start the walk at the deepest directory the prefix implies, so listing a
        // single partition never scans unrelated ones. Correctness still rests on
        // the `starts_with` filter below; the start directory is only an
        // optimization that can never skip a matching key.
        let mut stack = vec![self.walk_root(prefix)];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(OpenLocalListingDirectoryError::caused_by(dir, error).into());
                }
            };

            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(error) => {
                        return Err(AdvanceLocalListingDirectoryError::caused_by(
                            dir.clone(),
                            error,
                        )
                        .into());
                    }
                };
                let path = entry.path();
                let file_type = entry.file_type().await.map_err(|error| {
                    InspectLocalListingEntryError::caused_by(path.clone(), error)
                })?;
                if file_type.is_dir() {
                    stack.push(path);
                } else if is_temp_file_name(&entry.file_name()) {
                    // A crash mid-write can orphan a reserved-prefix temp file in
                    // the tree. It is not a real object, so never surface it.
                    continue;
                } else if let Some(key) = relative_key(&self.root, &path) {
                    keys.push(key);
                }
            }
        }

        keys.retain(|key| key.starts_with(prefix));
        keys.sort();
        Ok(keys)
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let path = self.key_path(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(ObjectNotFoundError::caused_by(key.to_owned(), error).into())
            }
            Err(error) => Err(RemoveLocalObjectError::caused_by(path, error).into()),
        }
    }
}

impl LocalStorage {
    /// The directory a `list(prefix)` walk should start from: the storage root
    /// joined with the prefix's leading run of complete (`/`-terminated) path
    /// segments. A trailing partial segment is excluded so it is matched by the
    /// `starts_with` filter instead. Descent stops at the first non-ordinary
    /// segment, leaving such (never-stored) prefixes to match nothing.
    fn walk_root(&self, prefix: &str) -> PathBuf {
        let mut dir = self.root.clone();
        if let Some((parents, _partial)) = prefix.rsplit_once('/') {
            for segment in parents.split('/') {
                if !is_plain_segment(segment) {
                    break;
                }
                dir.push(segment);
            }
        }
        dir
    }
}

fn relative_key(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut key = String::new();
    for component in relative.components() {
        let part = component.as_os_str().to_str()?;
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(part);
    }
    Some(key)
}

/// Whether `name` is one of the reserved temp filenames an atomic write uses.
fn is_temp_file_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(TEMP_PREFIX))
}

/// Publishes complete objects without a check-then-replace race.
///
/// Staging and publication share a filesystem; the temporary file owns failure cleanup.
/// See `docs/implementation.md` for the local publication policy.
async fn write_atomic(
    key: &str,
    target: PathBuf,
    bytes: Vec<u8>,
    overwrite: bool,
) -> Result<(), StorageError> {
    let key = key.to_owned();
    let error_path = target.clone();
    // tempfile exposes synchronous platform-specific no-clobber publication, so keep the
    // complete filesystem operation off the async worker rather than wrapping each syscall.
    tokio::task::spawn_blocking(move || -> Result<(), StorageError> {
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Windows canonicalization supplies the extended-length path form needed by
        // tempfile's native publication API. The destination itself need not exist.
        #[cfg(windows)]
        let target = parent
            .canonicalize()
            .map_err(|error| WriteLocalObjectError::caused_by(target.clone(), error))?
            .join(
                target
                    .file_name()
                    .expect("a validated storage key ends in an ordinary path component"),
            );
        #[cfg(windows)]
        let parent = target
            .parent()
            .expect("joining a file name to the canonical directory preserves its parent");
        let mut builder = Builder::new();
        builder.prefix(TEMP_PREFIX);
        #[cfg(unix)]
        builder.permissions(Permissions::from_mode(CREATION_PERMISSIONS));
        let mut staging = builder
            .tempfile_in(parent)
            .map_err(|error| WriteLocalObjectError::caused_by(target.clone(), error))?;
        staging
            .write_all(&bytes)
            .map_err(|error| WriteLocalObjectError::caused_by(target.clone(), error))?;

        let published = if overwrite {
            staging.persist(&target)
        } else {
            // A competing publication must fail here, not overwrite a winner after an exists check.
            staging.persist_noclobber(&target)
        };
        match published {
            Ok(_) => Ok(()),
            Err(error) if !overwrite && error.error.kind() == io::ErrorKind::AlreadyExists => {
                Err(ObjectAlreadyExistsError::caused_by(key, error.error).into())
            }
            Err(error) => Err(WriteLocalObjectError::caused_by(target, error.error).into()),
        }
    })
    .await
    .map_err(|error| WriteLocalObjectError::caused_by(error_path, error))?
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use ohno::ErrorExt as _;
    use tempfile::tempdir;

    use super::*;
    use crate::InvalidStorageKeyError;

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_then_get_roundtrips() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        storage.put("v1/folo/run.json", b"payload").await.unwrap();
        let bytes = storage.get("v1/folo/run.json").await.unwrap();

        assert_eq!(bytes, b"payload");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_compresses_the_object_at_rest() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let plain = br#"{"schema":1,"results":[]}"#;
        storage.put("v1/folo/run.json", plain).await.unwrap();

        // The file on disk is gzip, not the original plaintext: the bytes #260
        // transfers are the compressed ones, and `get` inflates them back.
        let on_disk = std::fs::read(dir.path().join("v1").join("folo").join("run.json")).unwrap();
        assert!(
            on_disk.starts_with(&[0x1f, 0x8b]),
            "stored bytes must be gzip"
        );
        assert_ne!(on_disk, plain, "the object is not stored verbatim");
        assert_eq!(cbh_codec::decompress(&on_disk).unwrap(), plain);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_rejects_a_legacy_plaintext_object() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        // An object written before compression existed is plaintext JSON. Reading
        // it back must fail loudly (the gzip magic is absent), never return raw
        // bytes that a later parse would misinterpret.
        let path = dir.path().join("v1").join("folo").join("run.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"schema":1}"#).unwrap();

        let error = storage.get("v1/folo/run.json").await.unwrap_err();

        assert!(error.find_source::<DecompressLocalObjectError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_leaves_no_temp_files_behind() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        storage.put("v1/folo/run.json", b"payload").await.unwrap();

        // The atomic rename consumes the temp file; only the object remains in
        // its directory, with no orphaned reserved-prefix temp file.
        let parent = dir.path().join("v1").join("folo");
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(&parent).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }

        assert_eq!(names, vec!["run.json".to_owned()]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn list_skips_reserved_temp_files() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a/1.json", b"1").await.unwrap();

        // Simulate a temp file orphaned beside the object by a crashed write.
        let orphan = dir
            .path()
            .join("v1")
            .join("a")
            .join(format!("{TEMP_PREFIX}123-456-0"));
        std::fs::write(&orphan, "partial").unwrap();

        // The orphan is never surfaced as a key.
        let keys = storage.list("v1/a/").await.unwrap();

        assert_eq!(keys, vec!["v1/a/1.json".to_owned()]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_is_write_once() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        storage.put("v1/folo/run.json", b"first").await.unwrap();
        let error = storage
            .put("v1/folo/run.json", b"second")
            .await
            .unwrap_err();

        assert_eq!(error.already_existing_key(), Some("v1/folo/run.json"));
        assert!(error.find_source::<ObjectAlreadyExistsError>().is_some());
        // The original object is left untouched.
        assert_eq!(storage.get("v1/folo/run.json").await.unwrap(), b"first");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_overwrite_replaces_an_existing_object() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        storage.put("v1/folo/run.json", b"first").await.unwrap();
        storage
            .put_overwrite("v1/folo/run.json", b"second")
            .await
            .unwrap();

        assert_eq!(storage.get("v1/folo/run.json").await.unwrap(), b"second");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_overwrite_creates_intermediate_directories() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        // No prior object at this key, and the parent directory does not exist.
        storage
            .put_overwrite("v1/folo/deep/run.json", b"only")
            .await
            .unwrap();

        assert_eq!(storage.get("v1/folo/deep/run.json").await.unwrap(), b"only");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_overwrite_rejects_traversal_key() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage
            .put_overwrite("v1/../escape.json", b"x")
            .await
            .unwrap_err();

        assert!(error.find_source::<InvalidStorageKeyError>().is_some());
    }

    #[cfg(windows)]
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_rejects_windows_absolute_segment() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        // A drive-absolute segment would otherwise rebind the path away from root.
        let error = storage
            .put("v1/C:\\Windows\\System32\\evil", b"x")
            .await
            .unwrap_err();

        assert!(error.find_source::<InvalidStorageKeyError>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_missing_key_reports_not_found() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.get("v1/missing.json").await.unwrap_err();

        assert!(error.is_not_found());
        let leaf = error.find_source::<ObjectNotFoundError>().unwrap();
        assert_eq!(leaf.key, "v1/missing.json");
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn list_returns_sorted_keys_under_prefix() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        storage.put("v1/a/2.json", b"2").await.unwrap();
        storage.put("v1/a/1.json", b"1").await.unwrap();
        storage.put("v1/b/3.json", b"3").await.unwrap();

        let keys = storage.list("v1/a/").await.unwrap();

        assert_eq!(
            keys,
            vec!["v1/a/1.json".to_owned(), "v1/a/2.json".to_owned()]
        );
    }

    #[test]
    fn walk_root_descends_through_complete_segments_only() {
        let storage = LocalStorage::new("root");
        let root = PathBuf::from("root");
        // Trailing complete segment -> descend into it.
        assert_eq!(storage.walk_root("v1/proj/"), root.join("v1").join("proj"));
        // Trailing partial segment -> stop at its parent, filter handles the rest.
        assert_eq!(
            storage.walk_root("v1/proj/cal"),
            root.join("v1").join("proj")
        );
        // A single segment with no `/` cannot be narrowed.
        assert_eq!(storage.walk_root("v1"), root);
        assert_eq!(storage.walk_root(""), root);
        // Descent halts at the first non-ordinary segment.
        assert_eq!(storage.walk_root("v1/../x/"), root.join("v1"));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn list_with_a_partial_segment_prefix_filters_precisely() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/proj/a.json", b"1").await.unwrap();
        storage.put("v1/proj/b.json", b"2").await.unwrap();
        storage.put("v1/other/c.json", b"3").await.unwrap();

        // The prefix ends mid-segment, so the walk starts at `v1/proj` and the
        // filter keeps only the key that actually begins with `v1/proj/a`.
        let keys = storage.list("v1/proj/a").await.unwrap();

        assert_eq!(keys, vec!["v1/proj/a.json".to_owned()]);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn list_missing_root_is_empty() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path().join("does-not-exist"));

        let keys = storage.list("v1/").await.unwrap();

        assert!(keys.is_empty());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_rejects_traversal_key() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.put("v1/../escape.json", b"x").await.unwrap_err();

        assert!(error.find_source::<InvalidStorageKeyError>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_rejects_traversal_key() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.get("../secret").await.unwrap_err();

        assert!(error.find_source::<InvalidStorageKeyError>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_on_a_directory_reports_io_error() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a/1.json", b"1").await.unwrap();

        // Reading the directory "v1/a" as a file is a non-NotFound I/O error.
        let error = storage.get("v1/a").await.unwrap_err();

        assert!(error.find_source::<ReadLocalObjectError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn list_on_a_file_root_reports_io_error() {
        let dir = tempdir().unwrap();
        let file_root = dir.path().join("not-a-dir");
        std::fs::write(&file_root, "x").unwrap();
        let storage = LocalStorage::new(&file_root);

        // Listing the whole store opens the root directly; a file root yields a
        // "not a directory" error rather than a benign "missing partition".
        let error = storage.list("").await.unwrap_err();

        assert!(
            error
                .find_source::<OpenLocalListingDirectoryError>()
                .is_some()
        );
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_blocked_by_an_existing_file_reports_io_error() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a", b"file").await.unwrap();

        // "v1/a" is a file, so creating it as a parent directory fails.
        let error = storage.put("v1/a/b.json", b"x").await.unwrap_err();

        assert!(
            error
                .find_source::<CreateLocalParentDirectoriesError>()
                .is_some()
        );
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_overwrite_blocked_by_an_existing_file_reports_io_error() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a", b"file").await.unwrap();

        let error = storage
            .put_overwrite("v1/a/b.json", b"x")
            .await
            .unwrap_err();

        assert!(
            error
                .find_source::<CreateLocalParentDirectoriesError>()
                .is_some()
        );
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn delete_removes_an_object_and_leaves_siblings() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a/clean.json", b"c").await.unwrap();
        storage.put("v1/a/dirty.json", b"d").await.unwrap();

        storage.delete("v1/a/dirty.json").await.unwrap();

        // The sibling survives and the deleted object is gone.
        assert_eq!(
            storage.list("v1/a/").await.unwrap(),
            vec!["v1/a/clean.json".to_owned()]
        );
        let error = storage.get("v1/a/dirty.json").await.unwrap_err();
        assert!(error.is_not_found());
        assert!(error.find_source::<ObjectNotFoundError>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn delete_missing_key_reports_not_found() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.delete("v1/missing.json").await.unwrap_err();

        assert!(error.is_not_found());
        let leaf = error.find_source::<ObjectNotFoundError>().unwrap();
        assert_eq!(leaf.key, "v1/missing.json");
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn delete_rejects_traversal_key() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.delete("v1/../escape.json").await.unwrap_err();

        assert!(error.find_source::<InvalidStorageKeyError>().is_some());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn delete_on_a_directory_reports_io_error() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a/1.json", b"1").await.unwrap();

        // Removing the directory "v1/a" as a file is a non-NotFound I/O error.
        let error = storage.delete("v1/a").await.unwrap_err();

        assert!(error.find_source::<RemoveLocalObjectError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
    }
}
