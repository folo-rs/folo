#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history analysis and shell crates, not a stable public API. \
              Exhaustive construction and matching by those in-workspace consumers \
              is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The stored data model for a benchmark run: a run reduced to a set of benchmark
//! results (each a stable identity plus its measured metrics), the run context that
//! situates it in time and against a commit, and the comparability rules that
//! partition runs into independently comparable series. Split out of
//! `cargo-bench-history` so this I/O-free data model is cheap to
//! mutation-test in isolation.
//!
//! Every type is re-exported flat from the crate root, so consumers write
//! `cbh_model::DiscriminantSet` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod benchmark_id;
mod bless;
mod comparability;
mod constants;
mod context;
mod metric;
mod run;

pub use benchmark_id::{BenchmarkId, BenchmarkIdPrefix, EmptyBenchmarkIdPrefix};
pub use bless::{BLESS_SCHEMA_VERSION, BlessingRecord};
pub use comparability::{DiscriminantSet, Engine, StorageKey, parse_key, sanitize_segment};
pub use constants::{OBJECTS_SEGMENT, STORAGE_VERSION};
pub use context::{
    EnvironmentInfo, EnvironmentProvider, GitInfo, RunContext, ToolchainInfo, detect_environment,
};
pub use metric::{Metric, MetricKind};
pub use run::{BenchmarkResult, MetricList, Run, SCHEMA_VERSION};
