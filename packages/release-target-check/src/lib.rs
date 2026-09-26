//! Supports candidate verification for the bootstrap publisher's command.
//!
//! Verification reads a caller-owned source snapshot before tag creation; publication
//! and disposal of that snapshot remain the caller's responsibility.

#![allow(
    missing_docs,
    reason = "This nonpublished library exposes implementation operations to its integration target, not a public API"
)]

pub use crp_publication::publication::candidate::{Metadata, Repository};
pub use crp_workspace::snapshot_command::{capture, git};
pub use run::run;

mod run;

#[cfg(test)]
::testing::set_allocator!();
