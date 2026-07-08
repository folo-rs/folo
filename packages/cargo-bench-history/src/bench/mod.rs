//! Benchmark-engine adapters: per-engine environment injection, the WSL env
//! propagation rule, and parsing of each engine's output into the model.
//!
//! Four engines are supported: Callgrind (via Gungraun, low-noise instruction
//! counts), Criterion (wall-clock timings), `alloc_tracker` (allocation counts
//! and bytes) and `all_the_time` (processor time). None is exact — every metric
//! carries run-to-run noise.

pub(crate) mod all_the_time;
pub(crate) mod alloc_tracker;
pub(crate) mod callgrind;
pub(crate) mod criterion;
mod env;
mod paths;
#[cfg(test)]
mod schema_roundtrip;

pub(crate) use all_the_time::parse_all_the_time_operation;
pub(crate) use alloc_tracker::parse_alloc_tracker_operation;
pub(crate) use callgrind::parse_callgrind_summary;
pub(crate) use criterion::parse_criterion_case;
pub(crate) use env::{injected_bench_env, usable_slope};
pub(crate) use paths::*;
