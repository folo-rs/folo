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
