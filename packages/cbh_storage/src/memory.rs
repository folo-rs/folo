use super::{Storage, StorageError, validate_key};

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
#[derive(Clone, Debug, Default)]
pub struct MemoryStorage {
    objects: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>,
}

impl MemoryStorage {
    /// Creates an empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the stored keys in sorted order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.objects.lock().unwrap().keys().cloned().collect()
    }
}

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;

    use super::*;

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
