//! [`LocalStorage`]: a [`Storage`] backed by a directory tree on the local
//! filesystem. Object keys map to relative paths under a configured root.

use std::io;
use std::path::{Component, Path, PathBuf};

use tokio::io::AsyncWriteExt;

use super::{Storage, StorageError};

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
        let mut path = self.root.clone();
        for segment in key.split('/') {
            if !is_plain_segment(segment) {
                return Err(StorageError::InvalidKey {
                    key: key.to_owned(),
                });
            }
            path.push(segment);
        }
        Ok(path)
    }
}

/// Returns `true` only if `segment` is a single ordinary path component, so it
/// can never escape or rebind the storage root when pushed onto a path.
fn is_plain_segment(segment: &str) -> bool {
    let mut components = Path::new(segment).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

impl Storage for LocalStorage {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.key_path(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(StorageError::Io)?;
        }
        // `create_new` makes the write fail rather than clobber an existing
        // object, upholding the write-once storage contract.
        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(StorageError::AlreadyExists {
                    key: key.to_owned(),
                });
            }
            Err(error) => return Err(StorageError::Io(error)),
        };
        file.write_all(bytes).await.map_err(StorageError::Io)?;
        // Tokio's `File` does not flush its buffer on drop, so an explicit
        // flush is required to guarantee the bytes reach the filesystem before
        // a subsequent `get` reads them back.
        file.flush().await.map_err(StorageError::Io)
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let path = self.key_path(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(StorageError::NotFound {
                key: key.to_owned(),
            }),
            Err(error) => Err(StorageError::Io(error)),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        let mut stack = vec![self.root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(StorageError::Io(error)),
            };

            while let Some(entry) = entries.next_entry().await.map_err(StorageError::Io)? {
                let file_type = entry.file_type().await.map_err(StorageError::Io)?;
                let path = entry.path();
                if file_type.is_dir() {
                    stack.push(path);
                } else if let Some(key) = relative_key(&self.root, &path) {
                    keys.push(key);
                }
            }
        }

        keys.retain(|key| key.starts_with(prefix));
        keys.sort();
        Ok(keys)
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use tempfile::tempdir;

    use super::*;

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
    async fn put_is_write_once() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        storage.put("v1/folo/run.json", b"first").await.unwrap();
        let error = storage
            .put("v1/folo/run.json", b"second")
            .await
            .unwrap_err();

        assert!(
            matches!(error, StorageError::AlreadyExists { .. }),
            "{error:?}"
        );
        // The original object is left untouched.
        assert_eq!(storage.get("v1/folo/run.json").await.unwrap(), b"first");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_maps_a_non_existence_open_error_to_io() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        // An interior NUL passes the plain-segment check but the OS rejects it at
        // open time with `InvalidInput` (not `AlreadyExists`), so it must surface
        // through the generic IO arm rather than the write-once arm.
        let error = storage.put("bad\0name", b"x").await.unwrap_err();

        assert!(matches!(error, StorageError::Io(_)), "{error:?}");
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

        assert!(
            matches!(error, StorageError::InvalidKey { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_missing_key_reports_not_found() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.get("v1/missing.json").await.unwrap_err();

        match error {
            StorageError::NotFound { key } => assert_eq!(key, "v1/missing.json"),
            other => panic!("unexpected error: {other:?}"),
        }
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

        assert!(
            matches!(error, StorageError::InvalidKey { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_rejects_traversal_key() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.get("../secret").await.unwrap_err();

        assert!(
            matches!(error, StorageError::InvalidKey { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn get_on_a_directory_reports_io_error() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a/1.json", b"1").await.unwrap();

        // Reading the directory "v1/a" as a file is a non-NotFound I/O error.
        let error = storage.get("v1/a").await.unwrap_err();

        assert!(matches!(error, StorageError::Io(_)), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn list_on_a_file_root_reports_io_error() {
        let dir = tempdir().unwrap();
        let file_root = dir.path().join("not-a-dir");
        std::fs::write(&file_root, "x").unwrap();
        let storage = LocalStorage::new(&file_root);

        let error = storage.list("v1/").await.unwrap_err();

        assert!(matches!(error, StorageError::Io(_)), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn put_blocked_by_an_existing_file_reports_io_error() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        storage.put("v1/a", b"file").await.unwrap();

        // "v1/a" is a file, so creating it as a parent directory fails.
        let error = storage.put("v1/a/b.json", b"x").await.unwrap_err();

        assert!(matches!(error, StorageError::Io(_)), "{error:?}");
    }
}
