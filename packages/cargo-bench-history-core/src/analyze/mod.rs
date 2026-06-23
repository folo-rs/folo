//! The analysis leaf modules.
//!
//! Pure functions that turn already-loaded result sets and git topology (passed in
//! as plain data) into a reconstructed timeline, detected findings, and a rendered
//! report. The shell crate's `analyze` orchestrator wires storage and git, then
//! calls into these.

pub mod discriminant;
pub mod findings;
pub mod report;
pub mod selection;
pub mod series;
pub mod stats;
