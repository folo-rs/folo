#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "Private application and maintainer-test wiring has no supported Rust API."
)]

//! Native source, process and archive execution for cargo-release-plan.

pub use native::Native;
pub use request::*;

mod archive;
pub mod command;
mod native;
mod request;
mod source;

#[cfg(test)]
::testing::set_allocator!();
