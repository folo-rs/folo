//! The analysis leaf modules.
//!
//! Pure functions that turn already-loaded result sets and git topology (passed in
//! as plain data) into a reconstructed timeline and detected findings. The shell
//! crate's `analyze` orchestrator wires storage and git, then calls into these, and
//! the `cbh_render` crate turns the findings into a rendered report.
//!
//! Every public type is re-exported flat from this module, so consumers write
//! `crate::detect::Finding` rather than reaching into a submodule.

pub(crate) mod discriminant;
#[cfg(any(test, feature = "private-test-util"))]
pub mod examples;
pub(crate) mod findings;
pub(crate) mod gate_log;
mod noise_gates;
pub(crate) mod parallel;
#[cfg(any(test, feature = "private-test-util"))]
pub(crate) mod recorded;
pub(crate) mod run_points;
#[cfg(any(test, feature = "private-test-util"))]
pub(crate) mod scatter;
pub(crate) mod selection;
pub(crate) mod series;
#[cfg(test)]
mod signal_validation;

pub use discriminant::{DiscriminantFilter, DiscriminantSetQuery};
pub use findings::{
    AnalysisConfig, AnalysisContext, AnalysisMode, Detection, Direction, Finding, FindingMethod,
    SeriesCensus, SeriesValue, Testability, UnjudgedReason, find_changes_spawned, short_commit,
    testability,
};
#[cfg(any(test, feature = "private-test-util"))]
pub use findings::{evaluate_with_log, find_changes};
// The gate types are compiled unconditionally because the detectors take a log by reference,
// so they appear in production signatures. Only the re-export is gated: outside this crate
// they are inspection machinery for the tests and the documentation figures, and nothing in
// the shell crate's own path constructs one.
#[cfg(any(test, feature = "private-test-util"))]
pub use gate_log::{Gate, GateLog, GateOutcome, GateStage};
pub use parallel::{balanced_chunk_sizes, worker_count};
pub use run_points::{MetricPoint, ResultPoints, RunPoints};
pub use selection::{DirtyAdmission, SelectedCommit, select_commits};
pub use series::{
    Blessing, BlessingPlacement, LoadedObject, Series, SeriesBuilder, SeriesFilter, SeriesPoint,
    apply_blessings, build_series, retain_present_at_context,
};
