//! [`StorageFacade`]: a static-dispatch enum over every storage backend.
//!
//! The [`Storage`](super::Storage) trait's methods return `impl Future` (an
//! RPITIT signature), which is not dyn-compatible, so callers select a backend
//! through this enum rather than a `Box<dyn Storage>`. [`build_storage`] maps a
//! resolved local path or the configured cloud backend to the corresponding
//! variant.

use std::path::Path;
use std::sync::Arc;

use azure_core::credentials::TokenCredential;
use azure_core::http::HttpClient;

use crate::config::{CloudStorageConfig, Config};
use crate::wiring::rebase;

use super::azure::AzureBlobStorage;
use super::local::LocalStorage;
use super::{Storage, StorageError};

/// A [`Storage`] backend selected at configuration time.
#[derive(Clone, Debug)]
pub(crate) enum StorageFacade {
    /// A local filesystem backend.
    Local(LocalStorage),
    /// An Azure Blob Storage backend.
    Azure(AzureBlobStorage),
}

impl Storage for StorageFacade {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put(key, bytes).await,
            Self::Azure(storage) => storage.put(key, bytes).await,
        }
    }

    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put_overwrite(key, bytes).await,
            Self::Azure(storage) => storage.put_overwrite(key, bytes).await,
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::Local(storage) => storage.get(key).await,
            Self::Azure(storage) => storage.get(key).await,
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        match self {
            Self::Local(storage) => storage.list(prefix).await,
            Self::Azure(storage) => storage.list(prefix).await,
        }
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.delete(key).await,
            Self::Azure(storage) => storage.delete(key).await,
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
/// # Errors
///
/// Returns [`StorageError::Config`] if no storage is selected (no `--local` and no
/// configured cloud backend), or if the selected cloud backend cannot be built —
/// for example an Azure backend with a non-HTTPS endpoint.
pub(crate) fn build_storage(
    local: Option<&Path>,
    config: &Config,
    base: &Path,
) -> Result<StorageFacade, StorageError> {
    if let Some(path) = local {
        return Ok(StorageFacade::Local(LocalStorage::new(rebase(
            base,
            path.to_path_buf(),
        ))));
    }
    match &config.storage {
        Some(CloudStorageConfig::Azure(azure)) => {
            Ok(StorageFacade::Azure(AzureBlobStorage::from_config(
                &azure.account,
                &azure.container,
                azure.endpoint.clone(),
            )?))
        }
        None => Err(StorageError::Config {
            message: "no storage configured: pass --local=<path> (or set \
                      CARGO_BENCH_HISTORY_STORAGE and pass a bare --local) or \
                      configure a cloud storage backend in the configuration file"
                .to_owned(),
        }),
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
            build_storage(Some(Path::new("./data")), &config, Path::new("/work")).unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)));
        assert!(format!("{storage:?}").contains("data"), "{storage:?}");
    }

    #[test]
    fn build_storage_local_overrides_a_configured_cloud_backend() {
        let config = config_with_storage("[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n");
        let storage =
            build_storage(Some(Path::new("./data")), &config, Path::new("/work")).unwrap();
        assert!(matches!(storage, StorageFacade::Local(_)), "{storage:?}");
    }

    #[test]
    fn build_storage_without_selection_or_config_is_a_config_error() {
        let config = Config::default();
        let error = build_storage(None, &config, Path::new("/work")).unwrap_err();
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
        let storage = build_storage(None, &config, Path::new("/work")).unwrap();
        assert!(matches!(storage, StorageFacade::Azure(_)));
    }
}
