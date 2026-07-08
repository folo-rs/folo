#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    reason = "this crate's `pub` items form an internal handoff boundary between the \
              cargo-bench-history sub-crates rather than a stable public API, so \
              exhaustive matching of its port value types by those in-workspace \
              consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The storage port and its backends. `analyze` and the mutating commands talk to a
//! single [`Storage`] trait; behind it sit a local-filesystem backend, an Azure Blob
//! backend, and a read-through caching backend that mirrors a cloud backend onto local
//! disk (using per-project cache-epoch markers to detect remote rewrites). The key-layout
//! and sanitization rules that keep those backends addressing the same objects live here
//! too, alongside an in-memory fake (behind `private-test-util`) so commands are testable
//! without a filesystem or cloud account. Split out of the `cargo-bench-history` shell so
//! the Azure and blob-storage dependencies are isolated for mutation testing.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_storage::StorageFacade` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod azure;
mod caching;
mod error;
mod facade;
mod github_oidc;
mod keys;
mod local;
#[cfg(any(test, feature = "private-test-util"))]
mod memory;
mod pending;
mod port;

pub use azure::AzureBlobStorage;
pub use caching::CachingStorage;
pub use error::StorageError;
pub use facade::{
    StorageFacade, StorageOverride, azure_backend_from_parts, build_storage, resolve_storage,
};
pub use keys::project_objects_prefix;
pub(crate) use keys::{cache_epoch_key, is_plain_segment, validate_key};
pub use local::LocalStorage;
#[cfg(any(test, feature = "private-test-util"))]
#[cfg_attr(docsrs, doc(cfg(feature = "private-test-util")))]
pub use memory::MemoryStorage;
pub(crate) use pending::PendingInvalidation;
pub use port::Storage;
