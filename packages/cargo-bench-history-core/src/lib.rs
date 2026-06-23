#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::module_name_repetitions,
    reason = "this crate is publish = false; its `pub` items form a handoff boundary \
              to the cargo-bench-history shell crate (and that crate's tests), not a \
              stable public API. Exhaustive construction and matching by those \
              in-workspace consumers is intended, and the deliberately descriptive \
              type names (DiscriminantSet, ReportFormat, RunContext, ...) read best \
              at the shell's use sites even though they restate their module name"
)]

//! Pure internals of [`cargo-bench-history`]: the stored data model, the
//! comparability/partitioning rules, and the analysis math and rendering.
//!
//! This crate deliberately performs **no I/O** — no storage, git, process, or
//! filesystem access. Everything that touches the outside world (storage
//! backends, git history, benchmark-output harvesting, the CLI) lives in the
//! `cargo-bench-history` shell crate, which depends on this one and calls these
//! leaf functions with already-loaded inputs.
//!
//! The split exists so the large, I/O-free half of the tool can be exercised by a
//! fast in-process unit-test suite that is cheap to mutation-test, instead of
//! re-paying the shell's git-subprocess-heavy integration suite for every mutant.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

pub mod analyze;
pub mod bless;
pub mod comparability;
pub mod context;
pub mod metric_events;
pub mod model;
