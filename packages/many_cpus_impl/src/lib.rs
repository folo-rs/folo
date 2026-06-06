#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]

//! Implementation crate for the [`many_cpus`] crate.
//!
//! This crate contains the entire implementation of `many_cpus`. The `many_cpus` crate
//! itself is a thin shell that re-exports the public-API subset of items defined here.
//!
//! **Do not depend on `many_cpus_impl` directly.** Anything beyond what `many_cpus`
//! re-exports is internal to this workspace and may change at any time, including in
//! patch releases. See [`docs/impl-crate-split.md`] for the broader convention.
//!
//! [`many_cpus`]: https://crates.io/crates/many_cpus
//! [`docs/impl-crate-split.md`]: https://github.com/folo-rs/folo/blob/main/docs/impl-crate-split.md

mod primitive_types;
mod processor;
mod processor_set;
mod processor_set_builder;
mod resource_quota;
mod system_hardware;

#[cfg(any(test, feature = "test-util"))]
pub mod fake;

pub use primitive_types::*;
pub use processor::*;
pub use processor_set::*;
pub use processor_set_builder::*;
pub use resource_quota::*;
pub use system_hardware::*;

pub mod pal;
