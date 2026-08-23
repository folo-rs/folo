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
//! releases. See the [`nm` implementation guide] for its role in the package
//! architecture and [`docs/impl-crate-split.md`] for the broader convention.
//!
//! [`nm`]: https://crates.io/crates/nm
//! [`nm` implementation guide]:
//!     https://github.com/folo-rs/folo/blob/main/packages/nm/docs/implementation.md
//! [`docs/impl-crate-split.md`]: https://github.com/folo-rs/folo/blob/main/docs/impl-crate-split.md

mod constants;
mod event;
mod event_builder;
mod event_name;
mod hashing;
mod magnitude;
mod observations;
mod observe;
mod publish_model;
mod pusher;
mod registries;
mod reports;

pub(crate) use constants::*;
pub use event::*;
pub use event_builder::*;
pub use event_name::*;
pub(crate) use hashing::*;
pub use magnitude::*;
pub(crate) use observations::*;
pub use observe::*;
pub use publish_model::*;
pub use pusher::*;
pub(crate) use registries::*;
pub use reports::*;
