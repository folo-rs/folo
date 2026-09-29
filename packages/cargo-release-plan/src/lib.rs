#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "Private executable and maintainer-test wiring has no supported Rust API."
)]

//! Internal wiring for the `cargo-release-plan` executable and maintainer tests.
//!
//! This package does not provide a supported Rust library API. Use its command line.

pub use cli::{Cli, EarlyExit};
pub use crp_versioning::CheckFormat;
pub use run::{RunInput, RunOutcome, run};

mod cli;
mod compatibility;
mod run;

#[cfg(test)]
::testing::set_allocator!();
