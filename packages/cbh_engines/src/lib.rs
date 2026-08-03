#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an internal handoff boundary between the \
              cargo-bench-history sub-crates rather than a stable public API, so \
              exhaustive construction and matching of its harvested-output value types \
              by those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The benchmark-engine adapters. Each supported engine (Callgrind, Criterion,
//! `alloc_tracker`, `all_the_time`) contributes the environment it needs to run and a
//! pure parser that turns its machine-readable output into the model, and the
//! benchmark-output port collects the summary files a run produced (filtered to those
//! this run wrote) so orchestration can harvest them. Split out of the
//! `cargo-bench-history` shell so this parser-heavy, deterministic code is isolated for
//! mutation testing.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_engines::FsBenchOutputSource` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod bench;
mod bench_output;

pub use bench::{
    AllTheTimeParseError, AllocTrackerParseError, CallgrindParseError, CriterionParseError,
    injected_bench_env, parse_all_the_time_operation, parse_alloc_tracker_operation,
    parse_callgrind_summary, parse_criterion_case,
};
pub use bench_output::{
    BenchOutputSource, FsBenchOutputSource, Harvest, RawCriterionCase, RawOperationFile, RawSummary,
};
