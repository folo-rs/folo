//! The stored data model for a benchmark run.
//!
//! A run is reduced to a set of benchmark results (each a stable identity plus its
//! measured metrics), the run context that situates it in time and against a
//! commit, and the comparability rules that partition runs into independently
//! comparable series.
//!
//! Every type is re-exported flat from this module, so consumers write
//! `crate::model::DiscriminantSet` rather than reaching into a submodule.

pub(crate) mod benchmark_id;
pub(crate) mod bless;
pub(crate) mod comparability;
pub(crate) mod constants;
pub(crate) mod context;
pub(crate) mod metric;
pub(crate) mod run;

pub use benchmark_id::{BenchmarkId, BenchmarkIdPrefix, EmptyBenchmarkIdPrefix};
pub use bless::{BLESS_SCHEMA_VERSION, BlessingRecord};
pub use comparability::{DiscriminantSet, Engine, sanitize_segment};
pub use constants::{L1_HITS_EVENT, LL_HITS_EVENT, RAM_HITS_EVENT};
pub use context::{
    EnvironmentInfo, EnvironmentProvider, GitInfo, RunContext, ToolchainInfo, detect_environment,
};
pub use metric::{Metric, MetricKind};
pub use run::{BenchmarkResult, Run, SCHEMA_VERSION};
