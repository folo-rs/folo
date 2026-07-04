//! The analysis leaf modules.
//!
//! Pure functions that turn already-loaded result sets and git topology (passed in
//! as plain data) into a reconstructed timeline, detected findings, and a rendered
//! report. The shell crate's `analyze` orchestrator wires storage and git, then
//! calls into these.
//!
//! Every public type is re-exported flat from this module, so consumers write
//! `crate::analyze::Finding` rather than reaching into a submodule.

pub(crate) mod discriminant;
pub(crate) mod findings;
pub(crate) mod parallel;
pub(crate) mod report;
pub(crate) mod run_points;
pub(crate) mod selection;
pub(crate) mod series;
#[cfg(test)]
mod signal_validation;
pub(crate) mod stats;

pub use discriminant::{DiscriminantSetQuery, FacetFilter, StorageKey, parse_key};
pub use findings::{
    AnalysisConfig, AnalysisContext, AnalysisMode, Direction, Finding, FindingMethod, SeriesValue,
    find_changes_spawned,
};
pub use parallel::{balanced_chunk_sizes, worker_count};
pub use report::{
    DEFAULT_SUMMARY_LIMIT, ReportFormat, ReportInput, SetSummary, format_value, render,
    render_markdown_summary,
};
pub use run_points::{MetricPoint, ResultPoints, RunPoints};
pub use selection::{SelectedCommit, select_commits};
pub use series::{
    Blessing, BlessingPlacement, LoadedObject, Series, SeriesBuilder, SeriesFilter, SeriesPoint,
    apply_blessings, build_series,
};
