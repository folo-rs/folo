//! [`StorageFacade`]: a static-dispatch enum over every storage backend.
//!
//! The [`Storage`](super::Storage) trait's methods return `impl Future` (an
//! RPITIT signature), which is not dyn-compatible, so callers select a backend
//! through this enum rather than a `Box<dyn Storage>`. [`build_storage`] maps a
//! resolved local path or the configured cloud backend to the corresponding
//! variant.

use std::path::Path;

use crate::config::{CloudStorageConfig, Config};
use crate::wiring::rebase;

use super::azure::AzureBlobStorage;
use super::local::LocalStorage;
use super::{Storage, StorageError};

/// A [`Storage`] backend selected at configuration time.
#[derive(Debug)]
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
/// for example an Azure backend with conflicting authentication settings.
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
        Some(CloudStorageConfig::Azure {
            account,
            container,
            endpoint,
            account_key,
            sas_token,
        }) => Ok(StorageFacade::Azure(AzureBlobStorage::from_config(
            account,
            container,
            endpoint.clone(),
            account_key.clone(),
            sas_token.clone(),
        )?)),
        None => Err(StorageError::Config {
            message: "no storage configured: pass --local <path> (or set \
                      CARGO_BENCH_HISTORY_STORAGE) or configure a cloud storage \
                      backend in the configuration file"
                .to_owned(),
        }),
    }
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
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn build_storage_for_azure_with_account_key_yields_an_azure_backend() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"http://127.0.0.1:10000/devstoreaccount1\"\n\
             account_key = \"Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==\"\n",
        );
        let storage = build_storage(None, &config, Path::new("/work")).unwrap();
        assert!(matches!(storage, StorageFacade::Azure(_)));
    }

    #[test]
    fn build_storage_for_azure_with_conflicting_auth_is_a_config_error() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n\
             account_key = \"a2V5\"\nsas_token = \"sig=x\"\n",
        );
        let error = build_storage(None, &config, Path::new("/work")).unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }
}
