//! Adapters for the private release-binaries and release-target-check executable protocols.
//!
//! These translate command and batch inputs for their publication and native owners.
//! The selected release workflow consumes these protocols; the module does not implement
//! another delivery engine or choose release policy.

pub use binaries::run_binaries;
pub use candidate::verify_candidate;

mod binaries;
mod binary_cli;
mod candidate;
mod candidate_cli;
mod model;
mod plan;
