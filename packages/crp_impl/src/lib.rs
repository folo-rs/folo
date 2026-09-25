#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "Maintainer integration tests exhaustively construct and match internal values; the supported facade owns the public API boundary."
)]
#![expect(
    clippy::module_name_repetitions,
    reason = "Subject modules namespace implementation operations; only the supported facade provides a flat public API."
)]
#![allow(
    missing_docs,
    reason = "Internal operations are exposed for maintainer integration tests, not as supported API contracts; the cargo-release-plan facade owns public documentation."
)]

//! Implementation of [`cargo-release-plan`](https://crates.io/crates/cargo-release-plan).
//!
//! Do not depend on this crate directly. Use the supported API re-exported by
//! `cargo-release-plan`; all other items are workspace-internal implementation details.

pub use check::CheckFormat;
pub use cli::{Cli, EarlyExit};
pub use errors::*;
pub use run::{RunInput, RunOutcome, run};
pub(crate) use text::{quote_path, short_commit};

/// Internal benchmark drivers that do not belong to the supported facade.
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
