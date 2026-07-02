//! [`StorageFacade`]: a static-dispatch enum over every storage backend.
//!
//! The [`Storage`](super::Storage) trait's methods return `impl Future` (an
//! RPITIT signature), which is not dyn-compatible, so callers select a backend
//! through this enum rather than a `Box<dyn Storage>`. [`build_storage`] maps a
//! resolved local path or the configured cloud backend to the corresponding
//! variant.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use azure_core::credentials::TokenCredential;
use azure_core::http::HttpClient;

use crate::config::{CloudStorageConfig, Config};
use crate::model::sanitize_segment;
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
            // A cached backend is attached only to `analyze`/`list`/`prune`, none of
            // which *store* objects; the object-writing commands (`run`/`backfill`/
            // `bless`) always build an uncached backend, so no `put` reaches the cache
            // decorator through the facade. (`prune` does mutate the cloud — it deletes
            // objects and flushes the invalidation marker — but it drives `delete`, never
            // `put`.) If caching is ever wired into an object-writing path, the decorator's
            // write-around semantics must be revisited here rather than silently relied on.
            Self::CachedAzure(_) => {
                unreachable!("a cached backend is never selected for an object-writing command")
            }
        }
    }

    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put_overwrite(key, bytes).await,
            Self::Azure(storage) => storage.put_overwrite(key, bytes).await,
            // Unreachable for the same reason as [`put`](Self::put): the cache is only
            // attached to commands that never store objects.
            Self::CachedAzure(_) => {
                unreachable!("a cached backend is never selected for an object-writing command")
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
    /// Wraps a plain [`Azure`](Self::Azure) backend in a read-through cache mirrored
    /// under `mirror_root`, leaving every other variant unchanged.
    ///
    /// This applies a `--cache` selection to a **test-injected** backend, which
    /// reaches the cloud through the [`StorageOverride`] seam and so never passes
    /// through the cache-wrapping [`build_storage`] performs for a config-resolved
    /// backend. The mirror roots directly at `mirror_root` — the injecting test owns
    /// its uniqueness — unlike the production path, which namespaces the mirror by
    /// backend identity to keep distinct backends from sharing one image.
    fn cached_at(self, mirror_root: PathBuf) -> Self {
        match self {
            Self::Azure(backend) => {
                Self::CachedAzure(CachingStorage::new(backend, LocalStorage::new(mirror_root)))
            }
            other @ (Self::Local(_) | Self::CachedAzure(_)) => other,
        }
    }

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
/// `<cache>/<account>/<container>` and faithfully images that one backend's cloud
/// namespace under identical keys. The mirror root is namespaced by backend identity
/// (account, then container) because two different Azure backends may share a single
/// `--cache` directory and their object keys collide (`v1/<project>/objects/...`);
/// rooting each mirror at the bare `<cache>` would let one backend's mirror be served
/// for another. Within a single backend's mirror, several projects still coexist, each
/// invalidated independently through its own marker. A cache is pointless with a
/// `--local` backend — reads are already local — so the CLI rejects that combination
/// outright (`--cache` conflicts with `--local`); a programmatic caller that sets both
/// still gets the local backend, with the cache ignored. A relative cache path rebases
/// against `base` exactly like `local`.
///
/// # Errors
///
/// Returns [`StorageError::Config`] if no storage is selected (no `--local` and no
/// configured cloud backend), or if the selected cloud backend cannot be built —
/// for example an Azure backend with a non-HTTPS endpoint.
pub(crate) fn build_storage(
    local: Option<&Path>,
    config: &Config,
    base: &Path,
    cache: Option<&Path>,
) -> Result<StorageFacade, StorageError> {
    if let Some(path) = local {
        return Ok(StorageFacade::Local(LocalStorage::new(rebase(
            base,
            path.to_path_buf(),
        ))));
    }
    match &config.storage {
        Some(CloudStorageConfig::Azure(azure)) => {
            let backend = AzureBlobStorage::from_config(
                &azure.account,
                &azure.container,
                azure.endpoint.clone(),
            )?;
            match cache {
                Some(cache_dir) => {
                    // The mirror faithfully images one cloud backend's namespace under
                    // identical keys. Different Azure backends (account/container) may
                    // share a `--cache` directory, and their object keys collide
                    // (`v1/<project>/objects/...`) with a matching genesis marker — so
                    // rooting the mirror at `<cache>` directly would let one backend's
                    // mirror be served for another (cross-backend cache poisoning).
                    // Namespace the mirror root by backend identity (account, then
                    // container) so each backend gets its own faithful mirror.
                    let mirror_root = rebase(base, cache_dir.to_path_buf())
                        .join(sanitize_segment(&azure.account))
                        .join(sanitize_segment(&azure.container));
                    Ok(StorageFacade::CachedAzure(CachingStorage::new(
                        backend,
                        LocalStorage::new(mirror_root),
                    )))
                }
                None => Ok(StorageFacade::Azure(backend)),
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

/// Selects the storage backend a cloud-read command (`analyze`/`list`/`prune`)
/// should use, honoring a test-injected override.
///
/// A `storage_override` takes precedence over configuration, mirroring how
/// [`build_storage`] lets `--local` override the configured cloud backend: an
/// injected backend reaches the cloud through the [`StorageOverride`] seam that
/// `build_storage` cannot reproduce. Because that override bypasses `build_storage`
/// it also bypasses the `--cache` wrapping `build_storage` performs, so this applies
/// the same wrapping here via [`StorageFacade::cached_at`] — keeping cached and
/// non-cached behavior identical whether the backend was configured or injected.
/// Without an override the configured backend is built as usual.
///
/// # Errors
///
/// Propagates any [`build_storage`] error when no override is supplied.
pub(crate) fn resolve_storage(
    storage_override: Option<StorageFacade>,
    local: Option<&Path>,
    config: &Config,
    base: &Path,
    cache: Option<&Path>,
    reporter: &dyn Reporter,
) -> Result<StorageFacade, StorageError> {
    match storage_override {
        Some(backend) => {
            reporter.note("storage backend: injected by test override");
            Ok(match cache {
                Some(cache_dir) => {
                    reporter.note(
                        "wrapping injected backend in a read-through cache so the override \
                         honors --cache like the configured path does",
                    );
                    backend.cached_at(rebase(base, cache_dir.to_path_buf()))
                }
                None => backend,
            })
        }
        None => build_storage(local, config, base, cache),
    }
}

/// A pre-built storage backend injected through
/// [`Overrides::storage_override`](crate::Overrides) so end-to-end tests can drive
/// commands against a backend that [`build_storage`] could not itself produce.
///
/// The wrapped [`StorageFacade`] is deliberately opaque: the type is public (so a
/// test in another crate can hold one) but carries no accessors, so the only thing
/// a caller can do with it is hand it back to a command. Test-support code (the
/// fake token credential, the certificate-trusting transport) stays in the tests;
/// only the assembled backend crosses the boundary.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct StorageOverride(pub(crate) StorageFacade);

/// Builds an Azure [`StorageOverride`] from already-constructed parts.
///
/// This is the command-level test seam: an end-to-end test supplies a token
/// credential and an HTTP client (for Azurite, a locally-faked Entra token and a
/// certificate-trusting transport) and receives a backend it can inject via
/// [`Overrides::storage_override`](crate::Overrides), bypassing the
/// config-to-[`build_storage`] path that could not produce such a backend.
///
/// # Errors
///
/// Returns [`StorageError::Config`] if the endpoint is not a valid base URL.
#[doc(hidden)]
pub fn azure_backend_from_parts(
    account: &str,
    container: &str,
    endpoint: Option<String>,
    credential: Arc<dyn TokenCredential>,
    http_client: Arc<dyn HttpClient>,
) -> Result<StorageOverride, StorageError> {
    let backend =
        AzureBlobStorage::from_parts(account, container, endpoint, credential, http_client)?;
    Ok(StorageOverride(StorageFacade::Azure(backend)))
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
        let storage =
            build_storage(Some(Path::new("./data")), &config, Path::new("/work"), None).unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)));
        assert!(format!("{storage:?}").contains("data"), "{storage:?}");
    }

    #[test]
    fn build_storage_local_overrides_a_configured_cloud_backend() {
        let config = config_with_storage("[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n");
        let storage =
            build_storage(Some(Path::new("./data")), &config, Path::new("/work"), None).unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
    }

    #[test]
    fn build_storage_without_selection_or_config_is_a_config_error() {
        let config = Config::default();
        let error = build_storage(None, &config, Path::new("/work"), None).unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn build_storage_for_azure_yields_an_azure_backend() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"https://devstoreaccount1.blob.core.windows.net\"\n",
        );
        let storage = build_storage(None, &config, Path::new("/work"), None).unwrap();
        assert!(matches!(storage, StorageFacade::Azure(_)));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn build_storage_for_azure_with_a_cache_yields_a_cached_backend() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"https://devstoreaccount1.blob.core.windows.net\"\n",
        );
        let storage = build_storage(
            None,
            &config,
            Path::new("/work"),
            Some(Path::new("./cache")),
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
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
    }

    /// A `CachedAzure` facade over an Azure endpoint. Only the backend construction
    /// is exercised here (which builds a real HTTP client but touches no network);
    /// the network dispatch arms are covered by the Azurite integration tests.
    fn azure_cache_facade() -> StorageFacade {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"https://devstoreaccount1.blob.core.windows.net\"\n",
        );
        build_storage(
            None,
            &config,
            Path::new("/work"),
            Some(Path::new("./cache")),
        )
        .unwrap()
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
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
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    #[cfg_attr(
        mutants,
        ignore = "A PendingInvalidation::take mutant turns this no-op into a real marker write against the configured Azure endpoint, which has no reachable backend here; the client's retry/backoff trips the mutation timeout. take()'s contract is caught fast by pending_invalidation_starts_unarmed."
    )]
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

    /// A plain (uncached) `Azure` facade, built offline. Constructing the backend
    /// spins up a real HTTP client but touches no network.
    fn plain_azure_facade() -> StorageFacade {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"https://devstoreaccount1.blob.core.windows.net\"\n",
        );
        build_storage(None, &config, Path::new("/work"), None).unwrap()
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn build_storage_namespaces_the_cache_mirror_by_backend_identity() {
        // Two distinct Azure backends (different account and container) share one
        // `--cache` directory. Their object keys collide (`v1/<project>/objects/...`),
        // so the mirror root MUST be namespaced by backend identity — otherwise one
        // backend's mirror could be served for the other (cross-backend cache
        // poisoning). Extract each mirror root from the facade's debug rendering (the
        // account/container also appear in the Azure backend's own fields, so a naive
        // substring test on the whole rendering would not isolate the mirror root),
        // then assert the two roots carry their own segments and differ.
        fn mirror_root(account: &str, container: &str) -> String {
            let config = config_with_storage(&format!(
                "[storage.azure]\naccount = \"{account}\"\ncontainer = \"{container}\"\n\
                 endpoint = \"https://{account}.blob.core.windows.net\"\n"
            ));
            let storage = build_storage(
                None,
                &config,
                Path::new("/work"),
                Some(Path::new("./cache")),
            )
            .unwrap();
            assert!(
                matches!(storage, StorageFacade::CachedAzure(_)),
                "{storage:?}"
            );
            let rendered = format!("{storage:?}");
            // The cache is the only `LocalStorage`, so its `root:` field appears once.
            rendered
                .split_once("root: \"")
                .expect("the cache backend renders its root")
                .1
                .split_once('"')
                .expect("the root path is quoted")
                .0
                .to_owned()
        }

        let one = mirror_root("alpha", "first");
        let two = mirror_root("beta", "second");
        assert!(
            one.contains("alpha") && one.contains("first"),
            "the mirror root is namespaced by backend identity: {one}"
        );
        assert!(
            two.contains("beta") && two.contains("second"),
            "the mirror root is namespaced by backend identity: {two}"
        );
        assert_ne!(
            one, two,
            "distinct Azure backends sharing a --cache dir must get distinct mirror roots"
        );
    }

    #[test]
    fn resolve_storage_without_an_override_builds_from_config() {
        // No override: the resolver defers to `build_storage`, which honors `--local`
        // and never announces a test injection.
        let reporter = RecordingReporter::new();
        let storage = resolve_storage(
            None,
            Some(Path::new("./data")),
            &Config::default(),
            Path::new("/work"),
            None,
            &reporter,
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
        assert!(
            !reporter.contains("injected by test override"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn resolve_storage_uses_an_uncached_override_verbatim() {
        // An override without `--cache` is used exactly as injected, with a note that
        // the backend came from a test override rather than configuration.
        let dir = tempdir().unwrap();
        let injected = StorageFacade::Local(LocalStorage::new(dir.path()));
        let reporter = RecordingReporter::new();
        let storage = resolve_storage(
            Some(injected),
            None,
            &Config::default(),
            Path::new("/work"),
            None,
            &reporter,
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
        assert!(
            reporter.contains("injected by test override"),
            "{:?}",
            reporter.notes()
        );
        assert!(
            !reporter.contains("read-through cache"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn resolve_storage_leaves_a_non_azure_override_unchanged_under_cache() {
        // Only an Azure override gains a mirror when `--cache` is set; every other
        // variant is returned unchanged (the CLI forbids `--local` with `--cache`, so
        // this guards the `cached_at` no-op arm rather than a real scenario).
        let dir = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let injected = StorageFacade::Local(LocalStorage::new(dir.path()));
        let reporter = RecordingReporter::new();
        let storage = resolve_storage(
            Some(injected),
            None,
            &Config::default(),
            Path::new("/work"),
            Some(cache.path()),
            &reporter,
        )
        .unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
        assert!(
            reporter.contains("read-through cache"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn resolve_storage_wraps_an_azure_override_to_honor_cache() {
        // A test-injected Azure backend bypasses the cache-wrapping `build_storage`
        // performs, so the resolver applies `--cache` itself — the injected backend
        // must come back as a `CachedAzure`, matching the configured path.
        let injected = plain_azure_facade();
        assert!(matches!(injected, StorageFacade::Azure(_)), "{injected:?}");
        let cache = tempdir().unwrap();
        let reporter = RecordingReporter::new();
        let storage = resolve_storage(
            Some(injected),
            None,
            &Config::default(),
            Path::new("/work"),
            Some(cache.path()),
            &reporter,
        )
        .unwrap();
        assert!(
            matches!(storage, StorageFacade::CachedAzure(_)),
            "{storage:?}"
        );
        assert!(
            reporter.contains("injected by test override")
                && reporter.contains("read-through cache"),
            "{:?}",
            reporter.notes()
        );
    }
}
