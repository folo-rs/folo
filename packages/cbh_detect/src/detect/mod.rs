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
pub(crate) mod findings;
pub(crate) mod parallel;
pub(crate) mod run_points;
pub(crate) mod selection;
pub(crate) mod series;
#[cfg(test)]
mod signal_validation;

pub use discriminant::{DiscriminantSetQuery, FacetFilter};
pub use findings::{
    AnalysisConfig, AnalysisContext, AnalysisMode, Direction, Finding, FindingMethod, SeriesValue,
    find_changes_spawned, short_commit,
};
pub use parallel::{balanced_chunk_sizes, worker_count};
pub use run_points::{MetricPoint, ResultPoints, RunPoints};
pub use selection::{SelectedCommit, select_commits};
pub use series::{
    Blessing, BlessingPlacement, LoadedObject, Series, SeriesBuilder, SeriesFilter, SeriesPoint,
    apply_blessings, build_series, retain_present_at_context,
};
