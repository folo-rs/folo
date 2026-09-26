#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::module_name_repetitions,
    reason = "Private application and maintainer-test wiring has no supported Rust API."
)]

//! Publication policy, remote delivery and recovery for cargo-release-plan.

pub(crate) use errors::*;
pub use output::PublicationOutput;

mod errors;
mod output;
pub mod publication;

#[cfg(test)]
::testing::set_allocator!();
