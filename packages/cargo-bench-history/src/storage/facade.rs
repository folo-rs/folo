//! [`StorageFacade`]: a static-dispatch enum over every storage backend.
//!
//! The [`Storage`](super::Storage) trait's methods return `impl Future` (an
//! RPITIT signature), which is not dyn-compatible, so callers select a backend
//! through this enum rather than a `Box<dyn Storage>`. [`build_storage`] maps a
//! [`StorageConfig`] to the corresponding variant.

use crate::config::{Config, StorageConfig};

use super::local::LocalStorage;
use super::{Storage, StorageError};

#[cfg(feature = "azure")]
use super::azure::AzureBlobStorage;

/// A [`Storage`] backend selected at configuration time.
#[derive(Debug)]
pub(crate) enum StorageFacade {
    /// A local filesystem backend.
    Local(LocalStorage),
    /// An Azure Blob Storage backend.
    #[cfg(feature = "azure")]
    Azure(AzureBlobStorage),
}

impl Storage for StorageFacade {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put(key, bytes).await,
            #[cfg(feature = "azure")]
            Self::Azure(storage) => storage.put(key, bytes).await,
        }
    }

    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        match self {
            Self::Local(storage) => storage.put_overwrite(key, bytes).await,
            #[cfg(feature = "azure")]
            Self::Azure(storage) => storage.put_overwrite(key, bytes).await,
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::Local(storage) => storage.get(key).await,
            #[cfg(feature = "azure")]
            Self::Azure(storage) => storage.get(key).await,
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        match self {
            Self::Local(storage) => storage.list(prefix).await,
            #[cfg(feature = "azure")]
            Self::Azure(storage) => storage.list(prefix).await,
        }
    }
}

/// Builds the storage backend the configuration selects.
///
/// # Errors
///
/// Returns [`StorageError::Config`] if the selected backend cannot be built —
/// for example an Azure backend with conflicting authentication settings, or an
/// Azure backend when the crate was built without the `azure` feature.
pub(crate) fn build_storage(config: &Config) -> Result<StorageFacade, StorageError> {
    match &config.storage {
        StorageConfig::Local { path } => Ok(StorageFacade::Local(LocalStorage::new(path.clone()))),
        #[cfg(feature = "azure")]
        StorageConfig::Azure {
            account,
            container,
            endpoint,
            account_key,
            sas_token,
        } => Ok(StorageFacade::Azure(AzureBlobStorage::from_config(
            account,
            container,
            endpoint.clone(),
            account_key.clone(),
            sas_token.clone(),
        )?)),
        #[cfg(not(feature = "azure"))]
        StorageConfig::Azure { .. } => Err(StorageError::Config {
            message: "Azure storage requires building cargo-bench-history with the `azure` feature"
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
        parse_config(storage).expect("test configuration should parse")
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
    }

    #[test]
    fn build_storage_for_local_yields_a_local_backend() {
        let config = config_with_storage("[storage.local]\npath = \"./data\"\n");
        let storage = build_storage(&config).expect("local storage always builds");
        assert!(matches!(storage, StorageFacade::Local(_)));
        assert!(format!("{storage:?}").contains("data"), "{storage:?}");
    }

    #[cfg(not(feature = "azure"))]
    #[test]
    fn build_storage_for_azure_without_feature_is_a_config_error() {
        let config = config_with_storage("[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n");
        let error = build_storage(&config).expect_err("azure needs the feature");
        match error {
            StorageError::Config { message } => {
                assert!(message.contains("azure"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[cfg(feature = "azure")]
    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn build_storage_for_azure_with_feature_yields_an_azure_backend() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"devstoreaccount1\"\ncontainer = \"bench-history\"\n\
             endpoint = \"http://127.0.0.1:10000/devstoreaccount1\"\n\
             account_key = \"Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==\"\n",
        );
        let storage = build_storage(&config).expect("azure storage builds");
        assert!(matches!(storage, StorageFacade::Azure(_)));
    }

    #[cfg(feature = "azure")]
    #[test]
    fn build_storage_for_azure_with_conflicting_auth_is_a_config_error() {
        let config = config_with_storage(
            "[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n\
             account_key = \"a2V5\"\nsas_token = \"sig=x\"\n",
        );
        let error = build_storage(&config).expect_err("conflicting auth");
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }
}
