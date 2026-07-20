#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]

//! Implementation crate for the [`nm`] crate.
//!
//! This crate contains the entire implementation of `nm`. The `nm` crate itself
//! is a thin shell that re-exports the public-API subset of items defined here.
//!
//! **Do not depend on `nm_impl` directly.** Anything beyond what `nm` re-exports
//! is internal to this workspace and may change at any time, including in patch
//! releases. See [`docs/impl-crate-split.md`] for the broader convention.
//!
//! [`nm`]: https://crates.io/crates/nm
//! [`docs/impl-crate-split.md`]: https://github.com/folo-rs/folo/blob/main/docs/impl-crate-split.md

mod constants;
mod data_types;
mod event;
mod event_builder;
mod hashing;
mod observations;
mod observe;
mod publish_model;
mod pusher;
mod registries;
mod reports;
mod sealed;

pub(crate) use constants::*;
pub use data_types::*;
pub use event::*;
pub use event_builder::*;
pub(crate) use hashing::*;
pub(crate) use observations::*;
pub use observe::*;
pub use publish_model::*;
pub use pusher::*;
pub(crate) use registries::*;
pub use reports::*;
pub(crate) use sealed::*;
