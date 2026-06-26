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
pub(crate) mod report;
pub(crate) mod selection;
pub(crate) mod series;
pub(crate) mod stats;

pub use discriminant::{DiscriminantSetQuery, FacetFilter, StorageKey, parse_key};
pub use findings::{
    AnalysisConfig, AnalysisContext, AnalysisMode, Direction, Finding, FindingMethod, SeriesValue,
    find_changes,
};
pub use report::{ReportFormat, ReportInput, SetSummary, render};
pub use selection::{SelectedCommit, select_commits};
pub use series::{
    Blessing, BlessingPlacement, LoadedObject, Series, SeriesFilter, SeriesPoint, apply_blessings,
    build_series,
};
