#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Maintain a long-lived history of benchmark results and analyze it for trends
//! that snapshot-only tooling cannot see.
//!
//! The tool stores one immutable result set per benchmark run (locally or, later,
//! in cloud blob storage), partitioned only by the factors that make results
//! fundamentally incomparable — project, engine system, target triple, and (for
//! hardware-dependent engines) a machine key — so that everything else stays
//! visible as a step in the timeline. The `analyze` command then reconstructs
//! per-benchmark series ordered by effective time and looks for regressions and
//! drift.
//!
//! The `run` command executes the configured benchmark engines (Callgrind via
//! Gungraun in this iteration), harvests their machine-readable output, and
//! stores one immutable result set per run in local storage. The `install` and
//! `analyze` command handlers are stubs that later iterations fill in.

mod bench;
mod bench_output;
mod cli;
mod commands;
mod comparability;
mod config;
mod context;
mod dispatch;
mod git;
mod host;
mod model;
mod probe;
mod process;
mod storage;
mod types;

pub use cli::Cli;
pub use comparability::{ComparabilityKey, EngineSystem, resolve_target_triple};
pub use config::{
    Config, ConfigError, EngineConfig, ProjectConfig, StorageConfig, default_template, parse_config,
};
pub use context::{
    CiInfo, CiProvider, GitInfo, RunContext, Timestamps, ToolchainInfo, detect_ci,
    resolve_effective_time,
};
pub use dispatch::run;
pub use model::{BenchmarkId, Metric, MetricKind, ResultRecord, ResultSet, SCHEMA_VERSION};
pub use storage::{LocalStorage, Storage, StorageError};
pub use types::{AnalyzeOptions, Command, InstallOptions, RunError, RunOptions, RunOutcome};
