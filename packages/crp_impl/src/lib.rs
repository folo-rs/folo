#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "Application code and maintainer tests exhaustively construct and match internal values; neither library target defines a supported Rust API."
)]
#![expect(
    clippy::module_name_repetitions,
    reason = "Subject modules namespace internal operations; the executable shell re-exports only its required wiring."
)]
#![allow(
    missing_docs,
    reason = "Rust operations are internal executable and maintainer-test wiring; supported contracts belong to the CLI and documented artifacts."
)]

//! Implementation of [`cargo-release-plan`](https://crates.io/crates/cargo-release-plan).
//!
//! Do not depend on this crate directly. Use the `cargo-release-plan` command line.
//! Both packages' Rust interfaces are internal application and maintainer-test wiring.

pub use check::CheckFormat;
pub use cli::{Cli, EarlyExit};
pub use errors::*;
pub use run::{RunInput, RunOutcome, run};
pub(crate) use text::{quote_path, short_commit};

/// Internal benchmark drivers that are not executable entry points.
#[cfg(any(test, feature = "private-test-util"))]
#[doc(hidden)]
pub mod __private {
    pub use crate::diff::benchmark_patch_rendering;
    pub use crate::lockfile::benchmark_lockfile_closures;
}

mod analysis_order;
pub mod anchor;
pub mod apply;
pub mod artifact_path;
mod check;
pub mod classify;
mod cli;
pub mod command;
mod compatibility;
mod diff;
mod errors;
pub mod expand;
pub mod git;
pub mod groups;
pub mod inherited;
mod inspect_plan;
pub mod lockfile;
pub mod manifest;
pub mod metadata;
pub mod packaging;
pub mod plan;
pub mod preview;
mod propose;
pub mod prospective;
pub mod publication;
mod report;
pub mod resolved;
mod run;
mod semver_targets;
mod text;
pub mod verbose;

#[cfg(test)]
::testing::set_allocator!();
