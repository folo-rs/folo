//! The storage abstraction: an immutable, list-by-prefix object store that both a
//! local filesystem and an Azure Blob container implement identically.

mod azure;
mod caching;
mod error;
mod facade;
mod github_oidc;
mod keys;
mod local;
#[cfg(test)]
mod memory;
mod pending;
mod port;

pub use error::StorageError;
pub(crate) use facade::{StorageFacade, build_storage, resolve_storage};
pub use facade::{StorageOverride, azure_backend_from_parts};
pub(crate) use keys::{cache_epoch_key, is_plain_segment, project_objects_prefix, validate_key};
#[cfg(test)]
pub(crate) use memory::MemoryStorage;
pub(crate) use pending::PendingInvalidation;
pub(crate) use port::Storage;
