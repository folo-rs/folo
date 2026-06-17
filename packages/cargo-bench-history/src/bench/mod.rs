//! Benchmark-engine adapters: per-engine environment injection, the WSL env
//! propagation rule, and parsing of each engine's output into the model.
//!
//! Two engines are supported: Callgrind (via Gungraun, deterministic instruction
//! counts) and Criterion (wall-clock timings).

pub(crate) mod callgrind;
pub(crate) mod criterion;

pub(crate) use callgrind::parse_callgrind_summary;
pub(crate) use criterion::parse_criterion_case;

use crate::comparability::EngineSystem;

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

/// The environment variables to inject so every supported engine emits
/// machine-readable output during the single `cargo bench` invocation.
///
/// This is the union of every engine's [`injected_env`]: Callgrind needs
/// `GUNGRAUN_SAVE_SUMMARY=pretty-json` so Gungraun writes the `summary.json`
/// files the tool harvests; Criterion writes `estimates.json` unconditionally, so
/// it contributes nothing. Duplicate names are de-duplicated, keeping the first.
pub(crate) fn injected_bench_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for engine in EngineSystem::ALL {
        for (name, value) in injected_env(engine) {
            if !env.iter().any(|(existing, _)| *existing == name) {
                env.push((name, value));
            }
        }
    }
    env
}

/// The environment variables to inject so an engine emits machine-readable output.
///
/// Callgrind needs `GUNGRAUN_SAVE_SUMMARY=pretty-json` so Gungraun writes the
/// `summary.json` files the tool harvests. Criterion writes `estimates.json`
/// unconditionally, so it needs nothing.
fn injected_env(engine: EngineSystem) -> Vec<(String, String)> {
    match engine {
        EngineSystem::Callgrind => {
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        }
        EngineSystem::Criterion => Vec::new(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn callgrind_injects_the_summary_flag() {
        let env = injected_env(EngineSystem::Callgrind);
        assert_eq!(
            env,
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        );
    }

    #[test]
    fn criterion_injects_nothing() {
        assert!(injected_env(EngineSystem::Criterion).is_empty());
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
}
