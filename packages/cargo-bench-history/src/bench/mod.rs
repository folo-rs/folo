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
#[cfg(test)]
mod schema_roundtrip;

pub(crate) use all_the_time::parse_all_the_time_operation;
pub(crate) use alloc_tracker::parse_alloc_tracker_operation;
pub(crate) use callgrind::parse_callgrind_summary;
pub(crate) use criterion::parse_criterion_case;

use crate::model::Engine;

/// Directory under the cargo target root where Gungraun writes its summaries.
pub(crate) const GUNGRAUN_DIR: &str = "gungraun";

/// File name Gungraun writes for each benchmark case's machine-readable summary.
pub(crate) const SUMMARY_FILE: &str = "summary.json";

/// Directory under the cargo target root where Criterion writes its results.
pub(crate) const CRITERION_DIR: &str = "criterion";

/// Directory name Criterion gives the most recent run of each benchmark case.
pub(crate) const CRITERION_NEW_DIR: &str = "new";

/// File name Criterion writes describing a benchmark case's identity.
pub(crate) const CRITERION_BENCHMARK_FILE: &str = "benchmark.json";

/// File name Criterion writes with a benchmark case's statistical estimates.
pub(crate) const CRITERION_ESTIMATES_FILE: &str = "estimates.json";

/// Directory under the cargo target root where `alloc_tracker` writes its
/// per-operation JSON files.
pub(crate) const ALLOC_TRACKER_DIR: &str = "alloc_tracker";

/// Directory under the cargo target root where `all_the_time` writes its
/// per-operation JSON files.
pub(crate) const ALL_THE_TIME_DIR: &str = "all_the_time";

/// The environment variables to inject so every supported engine emits
/// machine-readable output during the single `cargo bench` invocation.
///
/// This is the union of every engine's [`injected_env`]: Callgrind needs
/// `GUNGRAUN_SAVE_SUMMARY=pretty-json` so Gungraun writes the `summary.json`
/// files the tool harvests; Criterion writes `estimates.json` unconditionally, so
/// it contributes nothing. Duplicate names are de-duplicated, keeping the first.
pub(crate) fn injected_bench_env() -> Vec<(String, String)> {
    dedup_env(Engine::ALL.into_iter().flat_map(injected_env).collect())
}

/// Keeps the first occurrence of each variable name in `pairs`, dropping any later
/// repeat so two engines requesting the same variable do not inject it twice.
fn dedup_env(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for (name, value) in pairs {
        if !env.iter().any(|(existing, _)| *existing == name) {
            env.push((name, value));
        }
    }
    env
}

/// Returns the per-iteration slope only when it is a usable finite measurement.
///
/// The in-workspace engines (`alloc_tracker`, `all_the_time`) write a null slope
/// for a zero-iteration operation the workload could not run. Such an operation has
/// no comparable per-iteration figure, and a non-finite metric value cannot
/// round-trip through stored history: `serde_json` renders `NaN`/infinity as JSON
/// `null`, which then fails to deserialize back into the model's `f64`. Callers
/// therefore drop the operation rather than store a value that would corrupt the
/// run.
fn usable_slope(slope: Option<f64>) -> Option<f64> {
    slope.filter(|value| value.is_finite())
}

/// The environment variables to inject so an engine emits machine-readable output.
///
/// Callgrind needs `GUNGRAUN_SAVE_SUMMARY=pretty-json` so Gungraun writes the
/// `summary.json` files the tool harvests. Criterion, `alloc_tracker` and
/// `all_the_time` write their output unconditionally, so they need nothing.
fn injected_env(engine: Engine) -> Vec<(String, String)> {
    match engine {
        Engine::Callgrind => {
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        }
        Engine::Criterion | Engine::AllocTracker | Engine::AllTheTime => Vec::new(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn callgrind_injects_the_summary_flag() {
        let env = injected_env(Engine::Callgrind);
        assert_eq!(
            env,
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        );
    }

    #[test]
    fn criterion_injects_nothing() {
        assert!(injected_env(Engine::Criterion).is_empty());
    }

    #[test]
    fn auto_emitting_engines_inject_nothing() {
        // `alloc_tracker` and `all_the_time` write their JSON on `Session` drop,
        // so neither needs any environment variable injected.
        assert!(injected_env(Engine::AllocTracker).is_empty());
        assert!(injected_env(Engine::AllTheTime).is_empty());
    }

    #[test]
    fn combined_env_is_the_union_across_engines() {
        // The single `cargo bench` invocation gets the union of every engine's
        // env, deduplicated. Today only Callgrind contributes a variable.
        assert_eq!(
            injected_bench_env(),
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        );
    }

    #[test]
    fn dedup_env_keeps_the_first_value_and_drops_a_repeat_name() {
        // A later repeat of an existing name is dropped (first value wins) while a
        // distinct name is retained, exercising the equality test in the dedup.
        let pairs = vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned()),
            ("A".to_owned(), "shadowed".to_owned()),
        ];
        assert_eq!(
            dedup_env(pairs),
            vec![
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "2".to_owned()),
            ]
        );
    }

    #[test]
    fn usable_slope_keeps_only_present_finite_values() {
        // A present finite slope passes through; an absent one and any non-finite
        // one are rejected, so no non-finite figure can reach stored history.
        assert!(usable_slope(Some(1.5)).is_some());
        assert!(usable_slope(Some(0.0)).is_some());
        assert!(usable_slope(None).is_none());
        assert!(usable_slope(Some(f64::NAN)).is_none());
        assert!(usable_slope(Some(f64::INFINITY)).is_none());
        assert!(usable_slope(Some(f64::NEG_INFINITY)).is_none());
    }
}
