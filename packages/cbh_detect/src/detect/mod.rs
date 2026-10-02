//! The analysis leaf modules.
//!
//! Pure functions that turn already-loaded result sets and git topology (passed in
//! as plain data) into a reconstructed timeline and detected findings. The shell
//! crate's `analyze` orchestrator wires storage and git, then calls into these, and
//! the `cbh_render` crate turns the findings into a rendered report.
//!
//! Every public type is re-exported flat from this module, so consumers write
//! `crate::detect::Finding` rather than reaching into a submodule.

mod branch;
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
    AnalysisContext, AnalysisMode, BranchComparison, BranchComparisonTrace, BranchEvaluationTrace,
    BranchExcursion, BranchRangeRelation, BranchSeriesTrace, Detection, Direction, Finding,
    FindingMethod, SeriesCensus, SeriesValue, Testability, UnjudgedReason, evaluate_with_log,
    find_changes, find_changes_spawned, short_commit, testability,
};
pub use gate_log::{Gate, GateLog, GateOutcome, GateStage};
// The gating policy is a fixed set of named thresholds rather than a per-run
// configuration, so the constants are the public form of that policy: in-workspace
// consumers (the shell's series building, the documentation figures, the stress
// harness) read them directly instead of a tunable config object.
pub use noise_gates::*;
pub use parallel::{balanced_chunk_sizes, worker_count};
pub use run_points::{MetricPoint, ResultPoints, RunPoints};
pub use selection::{DirtyAdmission, SelectedCommit, select_commits};
pub use series::{
    BaseLevel, Blessing, BlessingPlacement, LoadedObject, Series, SeriesBuilder, SeriesFilter,
    SeriesPoint, apply_base_blessings, apply_blessings, attach_base_windows, build_series,
    retain_present_at_context,
};
