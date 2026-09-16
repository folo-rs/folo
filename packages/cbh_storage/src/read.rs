use std::future::{Future, ready};

use cbh_diag::Reporter;

use crate::{ReadOnlyStorageError, Storage, StorageError, StorageFacade};

/// A read-only view of a baseline and optional additional input.
///
/// Local input takes precedence for matching keys. Listings contain the union
/// of keys, with duplicates removed. Only object absence permits a read to fall
/// back to the baseline; all other failures remain errors.
///
/// The view never writes, overwrites or deletes objects in either store.
#[derive(Clone, Debug)]
pub struct ReadStorage<Baseline, Input> {
    baseline: Baseline,
    input: Option<Input>,
}

impl<Baseline, Input> ReadStorage<Baseline, Input> {
    /// Combines a baseline with optional additional read-only input.
    #[must_use]
    pub fn new(baseline: Baseline, input: Option<Input>) -> Self {
        Self { baseline, input }
    }
}

impl<Input> ReadStorage<StorageFacade, Input> {
    /// Synchronizes only the baseline's read-through cache.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while synchronizing that cache.
    // Trivial forwarding to the concrete baseline facade; cache policy is tested
    // independently with memory stores, and concrete cache I/O in integration tests.
    #[cfg_attr(test, mutants::skip)]
    pub async fn synchronize_cache(
        &self,
        project: &str,
        reporter: &dyn Reporter,
    ) -> Result<(), StorageError> {
        self.baseline.synchronize_cache(project, reporter).await
    }

    /// Reports cache activity for the baseline, excluding local input reads.
    // Trivial forwarding to the concrete baseline's diagnostic counters.
    #[cfg_attr(test, mutants::skip)]
    pub fn report_cache_tally(&self, reporter: &dyn Reporter) {
        self.baseline.report_cache_tally(reporter);
    }
}

impl<Baseline: Storage, Input: Storage> Storage for ReadStorage<Baseline, Input> {
    fn put(&self, _key: &str, _bytes: &[u8]) -> impl Future<Output = Result<(), StorageError>> {
        ready(Err(ReadOnlyStorageError::new("write").into()))
    }

    fn put_overwrite(
        &self,
        _key: &str,
        _bytes: &[u8],
    ) -> impl Future<Output = Result<(), StorageError>> + Send {
        ready(Err(ReadOnlyStorageError::new("overwrite").into()))
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        if let Some(input) = &self.input {
            match input.get(key).await {
                Ok(bytes) => return Ok(bytes),
                Err(error) if error.is_not_found() => {}
                Err(error) => return Err(error),
            }
        }
        self.baseline.get(key).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut keys = self.baseline.list(prefix).await?;
        if let Some(input) = &self.input {
            keys.extend(input.list(prefix).await?);
            keys.sort();
            keys.dedup();
        }
        Ok(keys)
    }

    fn delete(&self, _key: &str) -> impl Future<Output = Result<(), StorageError>> {
        ready(Err(ReadOnlyStorageError::new("delete").into()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use cbh_diag::RecordingReporter;
    use futures::executor::block_on;
    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::{CachingStorage, MemoryStorage, TestStorageError, cache_epoch_key};

    assert_impl_all!(
        ReadStorage<MemoryStorage, MemoryStorage>: Send, Sync, Clone, UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn queries_merge_keys_and_prefer_local_contents() {
        let baseline = MemoryStorage::new();
        let input = MemoryStorage::new();
        block_on(baseline.put("project/base", b"base")).unwrap();
        block_on(baseline.put("project/shared", b"old")).unwrap();
        block_on(input.put("project/shared", b"current")).unwrap();
        block_on(input.put("project/tip", b"tip")).unwrap();
        block_on(input.put("another/ignored", b"other")).unwrap();
        let storage = ReadStorage::new(baseline, Some(input));

        assert_eq!(
            block_on(storage.list("project/")).unwrap(),
            ["project/base", "project/shared", "project/tip"]
        );
        assert_eq!(block_on(storage.get("project/base")).unwrap(), b"base");
        assert_eq!(block_on(storage.get("project/shared")).unwrap(), b"current");
        assert_eq!(block_on(storage.get("project/tip")).unwrap(), b"tip");
        assert!(block_on(storage.get("absent")).unwrap_err().is_not_found());
    }

    #[test]
    fn no_input_preserves_baseline_reads() {
        let baseline = MemoryStorage::new();
        block_on(baseline.put("project/base", b"base")).unwrap();
        let storage = ReadStorage::new(baseline, None::<MemoryStorage>);
        assert_eq!(
            block_on(storage.list("project/")).unwrap(),
            ["project/base"]
        );
        assert_eq!(block_on(storage.get("project/base")).unwrap(), b"base");
        assert!(block_on(storage.get("absent")).unwrap_err().is_not_found());
    }

    #[test]
    fn mutation_attempts_never_reach_either_store() {
        let baseline = MemoryStorage::new();
        let input = MemoryStorage::new();
        block_on(baseline.put("key", b"baseline")).unwrap();
        block_on(input.put("key", b"input")).unwrap();
        let storage = ReadStorage::new(baseline.clone(), Some(input.clone()));
        for error in [
            block_on(storage.put("new", b"new")).unwrap_err(),
            block_on(storage.put_overwrite("key", b"replacement")).unwrap_err(),
            block_on(storage.delete("key")).unwrap_err(),
        ] {
            assert!(error.find_source::<ReadOnlyStorageError>().is_some());
            assert!(!error.is_not_found());
            assert_eq!(error.already_existing_key(), None);
        }
        assert_eq!(baseline.keys(), ["key"]);
        assert_eq!(input.keys(), ["key"]);
        assert_eq!(block_on(baseline.get("key")).unwrap(), b"baseline");
        assert_eq!(block_on(input.get("key")).unwrap(), b"input");
    }

    #[test]
    fn input_read_errors_do_not_fall_back_to_the_baseline() {
        let baseline = MemoryStorage::new();
        block_on(baseline.put("key", b"old")).unwrap();
        let storage = ReadStorage::new(baseline, Some(FailingStorage));
        let error = block_on(storage.get("key")).unwrap_err();
        assert!(error.find_source::<TestStorageError>().is_some());
    }

    #[test]
    fn a_local_hit_does_not_require_a_baseline_read() {
        let input = MemoryStorage::new();
        block_on(input.put("key", b"current")).unwrap();
        let storage = ReadStorage::new(FailingStorage, Some(input));
        assert_eq!(block_on(storage.get("key")).unwrap(), b"current");
        let error = block_on(storage.get("absent")).unwrap_err();
        assert!(error.find_source::<TestStorageError>().is_some());
    }

    #[test]
    fn neither_listing_failure_becomes_partial_success() {
        let baseline = MemoryStorage::new();
        block_on(baseline.put("baseline/key", b"base")).unwrap();
        let storage = ReadStorage::new(baseline, Some(FailingStorage));
        let error = block_on(storage.list("")).unwrap_err();
        assert!(error.find_source::<TestStorageError>().is_some());

        let input = MemoryStorage::new();
        block_on(input.put("input/key", b"input")).unwrap();
        let storage = ReadStorage::new(FailingStorage, Some(input));
        let error = block_on(storage.list("")).unwrap_err();
        assert!(error.find_source::<TestStorageError>().is_some());
    }

    #[test]
    fn cache_population_and_invalidation_leave_local_input_separate() {
        let baseline = MemoryStorage::new();
        let mirror = MemoryStorage::new();
        let input = MemoryStorage::new();
        let baseline_key = "v1/project/objects/base";
        let input_key = "v1/project/objects/tip";
        block_on(baseline.put(baseline_key, b"baseline")).unwrap();
        block_on(input.put(input_key, b"local")).unwrap();
        let cache = CachingStorage::new(baseline.clone(), mirror.clone());
        let storage = ReadStorage::new(cache, Some(input.clone()));
        let reporter = RecordingReporter::new();
        block_on(storage.baseline.synchronize("project", &reporter)).unwrap();
        assert_eq!(block_on(storage.get(baseline_key)).unwrap(), b"baseline");
        assert_eq!(block_on(storage.get(input_key)).unwrap(), b"local");
        assert!(block_on(mirror.get(input_key)).unwrap_err().is_not_found());
        assert_eq!(baseline.keys(), [baseline_key]);

        block_on(baseline.put_overwrite(&cache_epoch_key("project"), b"new epoch")).unwrap();
        block_on(storage.baseline.synchronize("project", &reporter)).unwrap();
        assert!(
            block_on(mirror.get(baseline_key))
                .unwrap_err()
                .is_not_found()
        );
        assert_eq!(block_on(input.get(input_key)).unwrap(), b"local");
        assert_eq!(block_on(baseline.get(baseline_key)).unwrap(), b"baseline");
    }

    /// An unavailable store whose failures cannot be mistaken for absent objects.
    #[derive(Clone, Debug)]
    struct FailingStorage;

    impl Storage for FailingStorage {
        fn put(&self, _: &str, _: &[u8]) -> impl Future<Output = Result<(), StorageError>> {
            ready(Err(TestStorageError::new().into()))
        }

        fn put_overwrite(
            &self,
            _: &str,
            _: &[u8],
        ) -> impl Future<Output = Result<(), StorageError>> + Send {
            ready(Err(TestStorageError::new().into()))
        }

        fn get(&self, _: &str) -> impl Future<Output = Result<Vec<u8>, StorageError>> + Send {
            ready(Err(TestStorageError::new().into()))
        }

        fn list(&self, _: &str) -> impl Future<Output = Result<Vec<String>, StorageError>> {
            ready(Err(TestStorageError::new().into()))
        }

        fn delete(&self, _: &str) -> impl Future<Output = Result<(), StorageError>> {
            ready(Err(TestStorageError::new().into()))
        }
    }
}
