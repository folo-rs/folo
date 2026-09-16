//! Verifies candidate source snapshots before release scripts create missing tags.
//!
//! This nonpublished controller utility reads a separate, caller-owned worktree;
//! GitHub operations and publication remain the responsibility of release orchestration.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![allow(
    missing_docs,
    reason = "This nonpublished library exposes implementation operations to its integration target, not a public API"
)]

pub use command::{capture, git};
pub use metadata::Metadata;
pub use repository::Repository;
pub use run::run;

mod cli;
mod command;
mod metadata;
mod repository;
mod run;
mod verification_repository;
mod verify;

#[cfg(test)]
mod verification_tests;
