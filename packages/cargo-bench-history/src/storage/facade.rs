//! [`StorageFacade`]: a static-dispatch enum over every storage backend.
//!
//! The [`Storage`](super::Storage) trait's methods return `impl Future` (an
//! RPITIT signature), which is not dyn-compatible, so callers select a backend
//! through this enum rather than a `Box<dyn Storage>`. [`build_storage`] maps a
//! resolved local path or the configured cloud backend to the corresponding
//! variant.

use std::path::Path;

use cargo_bench_history_core::model::sanitize_segment;

use crate::config::{CloudStorageConfig, Config};
use crate::report::Reporter;
use crate::wiring::rebase;

use super::azure::AzureBlobStorage;
use super::caching::CachingStorage;
use super::local::LocalStorage;
use super::{Storage, StorageError};

/// A [`Storage`] backend selected at configuration time.
#[derive(Clone, Debug)]
pub(crate) enum StorageFacade {
    /// A local filesystem backend.
    Local(LocalStorage),
    /// An Azure Blob Storage backend.
    Azure(AzureBlobStorage),
    /// An Azure Blob Storage backend behind a local read-through cache, selected
    /// when a `--cache` directory is set against the cloud backend.
    CachedAzure(CachingStorage<AzureBlobStorage, LocalStorage>),
}

impl Storage for StorageFacade {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put(key, bytes).await,
            Self::Azure(storage) => storage.put(key, bytes).await,
            // A cached backend is built only for the read-only `analyze`/`list`/`prune`
            // commands; the writing commands (`run`/`backfill`/`bless`) always build an
            // uncached backend, so no write ever reaches the cache decorator through the
            // facade. If caching is ever wired into a write path, the decorator's
            // write-around semantics must be revisited here rather than silently relied on.
            Self::CachedAzure(_) => {
                unreachable!("a cached backend is never selected for a writing command")
            }
        }
    }

    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put_overwrite(key, bytes).await,
            Self::Azure(storage) => storage.put_overwrite(key, bytes).await,
            // Unreachable for the same reason as [`put`](Self::put): the cache is only
            // ever attached to the read-only commands.
            Self::CachedAzure(_) => {
                unreachable!("a cached backend is never selected for a writing command")
            }
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::Local(storage) => storage.get(key).await,
            Self::Azure(storage) => storage.get(key).await,
            Self::CachedAzure(storage) => storage.get(key).await,
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        match self {
            Self::Local(storage) => storage.list(prefix).await,
            Self::Azure(storage) => storage.list(prefix).await,
            Self::CachedAzure(storage) => storage.list(prefix).await,
        }
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.delete(key).await,
            Self::Azure(storage) => storage.delete(key).await,
            Self::CachedAzure(storage) => storage.delete(key).await,
        }
    }
}

impl StorageFacade {
    /// Reconciles a read-through cache with the cloud before a load.
    ///
    /// A no-op for a backend with no cache (`Local`, or `Azure` without `--cache`);
    /// for `CachedAzure` it delegates to the decorator, which reads `project`'s
    /// invalidation marker and wipes the mirror if it has gone stale.
    ///
    /// # Errors
    ///
    /// Propagates any [`StorageError`] from the underlying synchronization.
    pub(crate) async fn synchronize_cache(
        &self,
        project: &str,
        reporter: &dyn Reporter,
    ) -> Result<(), StorageError> {
        match self {
            Self::Local(_) | Self::Azure(_) => Ok(()),
            Self::CachedAzure(storage) => storage.synchronize(project, reporter).await,
        }
    }

    /// Writes a fresh cache-invalidation marker if this command removed or
    /// overwrote a cloud object since the backend was built.
    ///
    /// A pure local backend has no marker and is a no-op; both cloud variants flush
    /// the underlying [`AzureBlobStorage`]'s pending-invalidation flag, so *other*
    /// machines' caches (e.g. the CI runner's) invalidate even when this writer
    /// holds no local cache of its own.
    ///
    /// # Errors
    ///
    /// Propagates any [`StorageError`] from writing the marker.
    pub(crate) async fn flush_pending_invalidation(
        &self,
        project: &str,
        reporter: &dyn Reporter,
    ) -> Result<(), StorageError> {
        match self {
            Self::Local(_) => Ok(()),
            Self::Azure(storage) => storage.flush_pending_invalidation(project, reporter).await,
            Self::CachedAzure(storage) => {
                storage
                    .inner()
                    .flush_pending_invalidation(project, reporter)
                    .await
            }
        }
    }

    /// Notes the read-through cache hit/miss tally at the end of a load, so a slow
    /// load can be diagnosed as a cold or invalidated mirror. A no-op unless a
    /// cache is in use.
    pub(crate) fn report_cache_tally(&self, reporter: &dyn Reporter) {
        if let Self::CachedAzure(storage) = self {
            storage.report_tally(reporter);
        }
    }
}

/// Builds the storage backend a command should use.
///
/// `local` is the resolved `--local` path (from
/// [`resolve_local_path`](crate::wiring::resolve_local_path)). When present it
/// selects local filesystem storage and **overrides** any configured cloud
/// backend; a relative path is taken relative to `base` (the process working
/// directory in production), so it resolves the same regardless of the process
/// current directory. When `local` is `None`, the single cloud backend configured
/// in `config` is used; if none is configured, that is an error.
///
/// `cache` is the resolved `--cache` directory (from
/// [`resolve_cache_path`](crate::wiring::resolve_cache_path)). It is meaningful
/// **only with the cloud backend**: when set against an Azure backend the backend
/// is wrapped in a [`CachingStorage`] whose mirror roots at
/// `<cache>/<project>` (so distinct projects keep distinct mirrors, each with its
/// own recorded epoch). A cache is pointless with a `--local` backend — reads are
/// already local — so the CLI rejects that combination outright (`--cache`
/// conflicts with `--local`); a programmatic caller that sets both still gets the
/// local backend, with the cache ignored. A relative cache path rebases against
/// `base` exactly like `local`.
///
/// # Errors
///
/// Returns [`StorageError::Config`] if no storage is selected (no `--local` and no
/// configured cloud backend), or if the selected cloud backend cannot be built —
/// for example an Azure backend with conflicting authentication settings.
pub(crate) fn build_storage(
    local: Option<&Path>,
    config: &Config,
    base: &Path,
    cache: Option<&Path>,
    project: &str,
) -> Result<StorageFacade, StorageError> {
    if let Some(path) = local {
        return Ok(StorageFacade::Local(LocalStorage::new(rebase(
            base,
            path.to_path_buf(),
        ))));
    }
    match &config.storage {
        Some(CloudStorageConfig::Azure {
            account,
            container,
            endpoint,
            account_key,
            sas_token,
        }) => {
            let azure = AzureBlobStorage::from_config(
                account,
                container,
                endpoint.clone(),
                account_key.clone(),
                sas_token.clone(),
            )?;
            match cache {
                Some(cache_dir) => {
                    // Root the mirror at <cache>/<project> so distinct projects do
                    // not share one mirror (and one recorded epoch); object keys are
                    // already project-qualified, but the recorded-epoch marker is not.
                    let mirror_root =
                        rebase(base, cache_dir.to_path_buf()).join(sanitize_segment(project));
                    Ok(StorageFacade::CachedAzure(CachingStorage::new(
                        azure,
                        LocalStorage::new(mirror_root),
                    )))
                }
                None => Ok(StorageFacade::Azure(azure)),
            }
        }
        None => Err(StorageError::Config {
            message: "no storage configured: pass --local=<path> (or set \
                      CARGO_BENCH_HISTORY_STORAGE and pass a bare --local) or \
                      configure a cloud storage backend in the configuration file"
                .to_owned(),
        }),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use tempfile::tempdir;

    use crate::config::parse_config;
    use crate::report::RecordingReporter;

    use super::*;

    fn config_with_storage(storage: &str) -> Config {
        parse_config(storage).unwrap()
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn local_facade_dispatches_every_storage_method() {
        let dir = tempdir().unwrap();
        let storage = StorageFacade::Local(LocalStorage::new(dir.path()));

        // `put` dispatches to the backend and writes the object.
        storage.put("v1/proj/run.json", b"first").await.unwrap();
        assert_eq!(storage.get("v1/proj/run.json").await.unwrap(), b"first");

        // `put_overwrite` must replace the existing contents through the facade;
        // overwriting with *different* bytes distinguishes a real dispatch from a
        // silent no-op.
        storage
            .put_overwrite("v1/proj/run.json", b"second")
            .await
            .unwrap();
        assert_eq!(storage.get("v1/proj/run.json").await.unwrap(), b"second");

        // `list` dispatches to the backend and surfaces the stored key.
        let keys = storage.list("v1/proj/").await.unwrap();
        assert_eq!(keys, vec!["v1/proj/run.json".to_owned()]);

        // `delete` dispatches to the backend and removes the object.
        storage.delete("v1/proj/run.json").await.unwrap();
        let error = storage.get("v1/proj/run.json").await.unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn build_storage_for_local_yields_a_local_backend() {
        let config = Config::default();
        let storage = build_storage(
            Some(Path::new("./data")),
            &config,
            Path::new("/work"),
            None,
            "proj",
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)));
        assert!(format!("{storage:?}").contains("data"), "{storage:?}");
    }

    #[test]
    fn build_storage_local_overrides_a_configured_cloud_backend() {
        let config = config_with_storage("[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n");
        let storage = build_storage(
            Some(Path::new("./data")),
            &config,
            Path::new("/work"),
            None,
            "proj",
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
    }

    #[test]
    fn build_storage_without_selection_or_config_is_a_config_error() {
        let config = Config::default();
        let error = build_storage(None, &config, Path::new("/work"), None, "proj").unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn build_storage_for_azure_with_account_key_yields_an_azure_backend() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"http://127.0.0.1:10000/devstoreaccount1\"\n\
             account_key = \"Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==\"\n",
        );
        let storage = build_storage(None, &config, Path::new("/work"), None, "proj").unwrap();
        assert!(matches!(storage, StorageFacade::Azure(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn build_storage_for_azure_with_a_cache_yields_a_cached_backend() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"http://127.0.0.1:10000/devstoreaccount1\"\n\
             account_key = \"Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==\"\n",
        );
        let storage = build_storage(
            None,
            &config,
            Path::new("/work"),
            Some(Path::new("./cache")),
            "proj",
        )
        .unwrap();
        assert!(
            matches!(storage, StorageFacade::CachedAzure(_)),
            "{storage:?}"
        );
    }

    #[test]
    fn build_storage_local_ignores_a_cache_directory() {
        // A cache is pointless with a local backend (reads are already local), so a
        // `--local` selection yields a plain local backend even with `--cache` set.
        let config = Config::default();
        let storage = build_storage(
            Some(Path::new("./data")),
            &config,
            Path::new("/work"),
            Some(Path::new("./cache")),
            "proj",
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
    }

    #[test]
    fn build_storage_for_azure_with_conflicting_auth_is_a_config_error() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n\
             account_key = \"a2V5\"\nsas_token = \"sig=x\"\n",
        );
        let error = build_storage(None, &config, Path::new("/work"), None, "proj").unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    /// A `CachedAzure` facade over the Azurite account-key endpoint. Only the
    /// backend construction is exercised here (which reads the wall clock for the
    /// SAS expiry but touches no network); the network dispatch arms are covered by
    /// the Azurite integration tests.
    fn azure_cache_facade() -> StorageFacade {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"http://127.0.0.1:10000/devstoreaccount1\"\n\
             account_key = \"Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==\"\n",
        );
        build_storage(
            None,
            &config,
            Path::new("/work"),
            Some(Path::new("./cache")),
            "proj",
        )
        .unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn report_cache_tally_notes_the_tally_for_a_cached_backend() {
        let storage = azure_cache_facade();
        let reporter = RecordingReporter::new();

        // No object was fetched, so the tally is all zeros — but the CachedAzure arm
        // must still emit it (the `Local`/`Azure` variants stay silent). Reading the
        // counters touches no network, so this needs no emulator.
        storage.report_cache_tally(&reporter);
        assert!(
            reporter.contains("served 0 objects from the local mirror and fetched 0 objects"),
            "{:?}",
            reporter.notes()
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    async fn flush_pending_invalidation_for_a_cached_backend_without_mutations_is_a_noop() {
        let storage = azure_cache_facade();
        let reporter = RecordingReporter::new();

        // Nothing armed the underlying backend's invalidation flag, so the flush
        // takes the early-return path and writes no marker — reaching the cloud only
        // when a mutation happened, so this needs no emulator. The CachedAzure arm
        // must still delegate to the wrapped backend rather than be a facade no-op.
        storage
            .flush_pending_invalidation("proj", &reporter)
            .await
            .unwrap();
        assert!(reporter.notes().is_empty(), "{:?}", reporter.notes());
    }
}
