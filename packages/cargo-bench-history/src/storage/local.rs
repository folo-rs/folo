//! [`LocalStorage`]: a [`Storage`] backed by a directory tree on the local
//! filesystem. Object keys map to relative paths under a configured root.

use std::io;
use std::path::{Path, PathBuf};

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

    fn key_path(&self, key: &str) -> PathBuf {
        let mut path = self.root.clone();
        for segment in key.split('/') {
            path.push(segment);
        }
        path
    }
}

impl Storage for LocalStorage {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.key_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(StorageError::Io)?;
        }
        tokio::fs::write(&path, bytes)
            .await
            .map_err(StorageError::Io)
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let path = self.key_path(key);
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
    async fn get_missing_key_reports_not_found() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        let error = storage.get("v1/missing.json").await.unwrap_err();

        match error {
            StorageError::NotFound { key } => assert_eq!(key, "v1/missing.json"),
            StorageError::Io(error) => panic!("unexpected io error: {error}"),
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
}
