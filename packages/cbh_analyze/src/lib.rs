#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The `analyze`-family orchestration: resolve which stored runs make up each
//! benchmark's history from live git topology, reconstruct the series, and report the
//! notable changes, plus the sibling `list`, `prune`, `examine`, and `bless`/`unbless`
//! commands that share the same loading and selection machinery.
//!
//! Unlike a snapshot tool, `analyze` orders a series by *git history* rather than by
//! ingest time (see the `analyze` command in `DESIGN.md`): it resolves the target ref's
//! first-parent ancestry, splits it at the merge-base with a base branch, and admits
//! dirty (uncommitted-tree) snapshots only on the target side of that split. The pure
//! logic (selection, series reconstruction, finding detection, report rendering) stays
//! sync and Miri-safe; only the git queries and object loads touch async ports. The
//! command entry points wire the real adapters; the `analyze_with`-style orchestrators
//! they call are storage- and git-generic so the in-memory tests can drive them. Split
//! out of the `cargo-bench-history` shell so this orchestration layer is cheap to
//! mutation-test in isolation.
//!
//! Every command entry point is re-exported flat from the crate root, so the shell
//! writes `cbh_analyze::analyze` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod bless;
mod dataset;
mod examine;
mod facets;
mod history;
mod list;
mod load;
mod pipeline;
mod prune;
mod selection;
mod window;

pub use bless::{bless, unbless};
pub(crate) use cbh_analysis::{Series, SeriesFilter, StorageKey, apply_blessings};
pub(crate) use cbh_render::{ReportFormat, format_value};
pub(crate) use dataset::{empty_history_hint, select_dataset};
pub use examine::execute as examine;
pub(crate) use facets::{AutoFacets, resolve_facets};
pub(crate) use history::{
    DirtyTipPolicy, ResolvedHistory, dirty_base_exception_warning, resolve_base_name,
    resolve_base_ref, resolve_history,
};
pub use list::execute as list;
pub(crate) use load::{RunIndex, facet_filtered_candidates};
pub use pipeline::execute as analyze;
pub(crate) use pipeline::{detect_auto_facets, resolve_now};
pub use prune::execute as prune;
pub(crate) use selection::Selection;
pub(crate) use window::{WindowEdge, parse_since, parse_until, window_excludes};
