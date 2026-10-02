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

pub use archive::{stage_executable_for_test, write_archive_for_test};
pub use native::Native;
pub use request::*;
pub use zip_writer::benchmark_archive;

mod archive;
pub mod command;
mod native;
mod request;
mod source;
mod zip_writer;

#[cfg(test)]
::testing::set_allocator!();
