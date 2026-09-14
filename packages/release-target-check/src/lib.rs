//! Verifies candidate source snapshots before release scripts create missing tags.
//!
//! This nonpublished controller utility reads a separate, caller-owned worktree;
//! GitHub operations and publication remain the responsibility of release orchestration.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

pub use run::run;

mod cli;
mod command;
mod metadata;
mod repository;
mod run;
mod verify;
