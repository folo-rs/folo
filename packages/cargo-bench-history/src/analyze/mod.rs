//! The `analyze` command: resolve which stored runs make up each benchmark's
//! history from live git topology, reconstruct the series, and report the notable
//! changes.
//!
//! Unlike a snapshot tool, `analyze` orders a series by *git history* rather than
//! by ingest time (see the `analyze` command in `DESIGN.md`): it resolves the
//! target ref's first-parent
//! ancestry, splits it at the merge-base with a base branch, and admits dirty
//! (uncommitted-tree) snapshots only on the target side of that split. The pure
//! logic (selection, series reconstruction, finding detection, report rendering)
//! stays sync and Miri-safe; only the git queries and object loads touch async
//! ports. [`execute`] wires the real adapters; [`analyze_with`](pipeline::analyze_with) is the
//! storage- and git-generic orchestrator the in-memory tests drive.

pub(crate) mod bless;
pub(crate) mod examine;
pub(crate) mod list;
mod pipeline;
pub(crate) mod prune;

pub(crate) use cargo_bench_history_core::analyze::{
    ReportFormat, Series, SeriesFilter, StorageKey, apply_blessings, format_value,
};
pub(crate) use pipeline::{
    AutoFacets, DirtyTipPolicy, ResolvedHistory, RunIndex, Selection, WindowEdge,
    detect_auto_facets, dirty_base_exception_warning, empty_history_hint, execute,
    facet_filtered_candidates, parse_since, parse_until, resolve_base_name, resolve_base_ref,
    resolve_facets, resolve_history, resolve_now, select_dataset, window_excludes,
};
