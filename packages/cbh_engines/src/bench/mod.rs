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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_model::{BenchmarkResult, Engine, IntervalSupport};

    use crate::bench::{
        parse_all_the_time_operation, parse_alloc_tracker_operation, parse_callgrind_summary,
        parse_criterion_case,
    };

    const CRITERION_BENCHMARK: &str =
        include_str!("../../tests/fixtures/criterion/std_instant/benchmark.json");
    const CRITERION_ESTIMATES: &str =
        include_str!("../../tests/fixtures/criterion/std_instant/estimates.json");
    const CALLGRIND_SUMMARY: &str =
        include_str!("../../tests/fixtures/callgrind/single_unparametrized.summary.json");
    const ALLOC_TRACKER_SINGLE_SPAN: &str =
        include_str!("../../tests/fixtures/alloc_tracker/allocate_vec.json");
    const ALLOC_TRACKER_MULTI_SPAN: &str =
        include_str!("../../tests/fixtures/alloc_tracker/allocate_vec_dispersion.json");
    const ALL_THE_TIME_SINGLE_SPAN: &str =
        include_str!("../../tests/fixtures/all_the_time/read_cell.json");
    const ALL_THE_TIME_MULTI_SPAN: &str =
        include_str!("../../tests/fixtures/all_the_time/read_cell_dispersion.json");

    #[test]
    fn engine_interval_support_matches_adapter_output() {
        for engine in Engine::ALL {
            match engine {
                Engine::Criterion => {
                    assert_eq!(engine.interval_support(), IntervalSupport::Always);
                    let record =
                        parse_criterion_case(CRITERION_BENCHMARK, CRITERION_ESTIMATES).unwrap();
                    assert_all_metrics_have_intervals(&record);
                }
                Engine::Callgrind => {
                    assert_eq!(engine.interval_support(), IntervalSupport::Never);
                    let record = parse_callgrind_summary(CALLGRIND_SUMMARY).unwrap();
                    assert_no_metrics_have_intervals(&record);
                }
                Engine::AllocTracker => {
                    assert_eq!(engine.interval_support(), IntervalSupport::MultiSpanOnly);
                    let single_span = parse_alloc_tracker_operation(ALLOC_TRACKER_SINGLE_SPAN)
                        .unwrap()
                        .unwrap();
                    let multi_span = parse_alloc_tracker_operation(ALLOC_TRACKER_MULTI_SPAN)
                        .unwrap()
                        .unwrap();
                    assert_no_metrics_have_intervals(&single_span);
                    assert_all_metrics_have_intervals(&multi_span);
                }
                Engine::AllTheTime => {
                    assert_eq!(engine.interval_support(), IntervalSupport::MultiSpanOnly);
                    let single_span = parse_all_the_time_operation(ALL_THE_TIME_SINGLE_SPAN)
                        .unwrap()
                        .unwrap();
                    let multi_span = parse_all_the_time_operation(ALL_THE_TIME_MULTI_SPAN)
                        .unwrap()
                        .unwrap();
                    assert_no_metrics_have_intervals(&single_span);
                    assert_all_metrics_have_intervals(&multi_span);
                }
            }
        }
    }

    fn assert_all_metrics_have_intervals(record: &BenchmarkResult) {
        assert!(!record.metrics.is_empty());
        for metric in &record.metrics {
            assert!(metric.interval_low.is_some());
            assert!(metric.interval_high.is_some());
        }
    }

    fn assert_no_metrics_have_intervals(record: &BenchmarkResult) {
        assert!(!record.metrics.is_empty());
        for metric in &record.metrics {
            assert!(metric.interval_low.is_none());
            assert!(metric.interval_high.is_none());
        }
    }
}
