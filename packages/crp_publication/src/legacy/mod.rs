//! Temporary adapters for the bootstrap publisher's private executable protocols.
//!
//! Keep these only while the old workflow calls `release-binaries` and
//! `release-target-check`. The operational cutover removes this module and both
//! nonpublished shells. Native execution and publication decisions use their
//! ordinary owners; this module does not implement another delivery engine.

pub use binaries::run_binaries;
pub use candidate::verify_candidate;

mod binaries;
mod binary_cli;
mod candidate;
mod candidate_cli;
mod model;
mod plan;
