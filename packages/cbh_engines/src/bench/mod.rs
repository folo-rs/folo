//! Benchmark-engine adapters: per-engine environment injection, the WSL env
//! propagation rule, and parsing of each engine's output into the model.
//!
//! Four engines are supported: Callgrind (via Gungraun, low-noise instruction
//! counts), Criterion (wall-clock timings), `alloc_tracker` (allocation counts
//! and bytes) and `all_the_time` (processor time). None is exact — every metric
//! carries run-to-run noise.

mod all_the_time;
mod alloc_tracker;
mod callgrind;
mod criterion;
mod env;
mod paths;
#[cfg(test)]
mod schema_roundtrip;

pub use all_the_time::{AllTheTimeParseError, parse_all_the_time_operation};
pub use alloc_tracker::{AllocTrackerParseError, parse_alloc_tracker_operation};
pub use callgrind::{CallgrindParseError, parse_callgrind_summary};
pub use criterion::{CriterionParseError, parse_criterion_case};
pub use env::injected_bench_env;
pub(crate) use env::usable_slope;
pub(crate) use paths::*;
