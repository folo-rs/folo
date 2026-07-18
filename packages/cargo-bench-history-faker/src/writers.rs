//! The synthetic benchmark-output writers and their pure value cores.
//!
//! Each engine has a pure value core that builds the on-disk document as a value
//! (constructed without touching the filesystem, so it is usable from unit tests
//! and Miri) and a thin `write_*` wrapper that serializes it into the cargo target
//! tree exactly where the real engine would, honoring `CARGO_TARGET_DIR`.
//!
//! None of this is a stable API — see the crate root.

use std::path::Path;

/// The `module_path` of the committed `single` Callgrind fixture, reproduced so the
/// `single`/`single-alt-pkg` presets keep the exact `BenchmarkId` identity that
/// existing downstream assertions expect.
const SINGLE_MODULE_PATH: &str =
    "fast_time_timestamp_performance_cg::timestamp_capture::timestamp_capture_std_now";
/// The `function_name` of the committed `single` fixture.
const SINGLE_FUNCTION: &str = "timestamp_capture_std_now";
/// The `module_path` of the committed `parametrized` fixture.
const PARAMETRIZED_MODULE_PATH: &str = "fast_time_timestamp_performance_cg::timestamp_capture::timestamp_capture_instant_saturating_duration_since";
/// The `function_name` of the committed `parametrized` fixture.
const PARAMETRIZED_FUNCTION: &str = "timestamp_capture_instant_saturating_duration_since";
/// The `package_dir` both fixtures report; its final component (`fast_time`)
/// becomes the package segment of the parsed `BenchmarkId`.
const SINGLE_PACKAGE_DIR: &str = "/mnt/c/Source/folo/packages/fast_time";
/// A different `package_dir` keeping the same `module_path`/`function_name`,
/// simulating an equally named bench target in another package (final component
/// `other_pkg`).
const ALT_PACKAGE_DIR: &str = "/work/packages/other_pkg";

/// A parsed Criterion case request: its identity and slope estimate (nanoseconds).
#[derive(Debug)]
#[non_exhaustive]
pub struct CriterionCase {
    /// The Criterion group id.
    pub group: String,
    /// The Criterion function id.
    pub function: String,
    /// The optional parameter value (empty for an unparametrized case).
    pub value: String,
    /// The wall-clock slope estimate, in nanoseconds.
    pub nanos: f64,
}

/// A parsed `alloc_tracker` operation request.
///
/// Carries its name, per-iteration means, and optional per-metric confidence
/// intervals `((bytes_low, bytes_high), (count_low, count_high))`. When the
/// intervals are present the operation file additionally records slopes and span
/// count for both metrics, mirroring dispersion-bearing output.
#[derive(Debug)]
#[non_exhaustive]
pub struct AllocOperation {
    /// The operation name.
    pub operation: String,
    /// Mean bytes allocated per iteration.
    pub mean_bytes: u64,
    /// Mean allocation count per iteration.
    pub mean_allocations: u64,
    /// Optional per-metric confidence intervals.
    pub intervals: Option<((f64, f64), (f64, f64))>,
}

/// A parsed `all_the_time` operation request.
///
/// Carries its name, per-iteration mean, and an optional confidence interval
/// `(low, high)` in nanoseconds. When the interval is present the operation file
/// additionally records a slope and span count, mirroring dispersion-bearing
/// output.
#[derive(Debug)]
#[non_exhaustive]
pub struct TimeOperation {
    /// The operation name.
    pub operation: String,
    /// Mean processor-time nanoseconds per iteration.
    pub mean_nanos: u64,
    /// Optional confidence interval.
    pub interval: Option<(f64, f64)>,
}

/// Builds a schema-v6 Gungraun/Callgrind `summary.json` document as a string.
///
/// The pure value core behind [`write_callgrind_summary`]. It emits only the
/// fields the `cargo-bench-history` parser reads — the identity (`module_path`,
/// `function_name`, `id`, `package_dir`) and the three tracked events `Ir`/`Bc`/`Bi`
/// under `profiles[].summaries.total.summary.Callgrind` — which is enough for a
/// faithful round-trip; the real capture's many derived and cache-simulation events
/// are parsed-but-ignored, so they are omitted.
#[must_use]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn callgrind_summary(
    module_path: &str,
    function_name: &str,
    id: Option<&str>,
    package_dir: Option<&str>,
    ir: u64,
    bc: u64,
    bi: u64,
) -> String {
    let metric = |value: u64| serde_json::json!({ "metrics": { "Left": { "Int": value } } });
    serde_json::json!({
        "version": "6",
        "module_path": module_path,
        "function_name": function_name,
        "id": id,
        "package_dir": package_dir,
        "profiles": [{
            "summaries": {
                "total": {
                    "summary": {
                        "Callgrind": {
                            "Ir": metric(ir),
                            "Bc": metric(bc),
                            "Bi": metric(bi),
                        }
                    }
                }
            }
        }],
    })
    .to_string()
}

/// Builds one of the fixed-parameter Callgrind presets by name.
///
/// The three presets reproduce the identity and `Ir`/`Bc`/`Bi` of the committed
/// real fixtures so existing downstream `BenchmarkId` assertions stay green:
///
/// * `single` — unparametrized, `Ir` = 36, package `fast_time`.
/// * `parametrized` — id `two_instants`, `Ir` = 87, package `fast_time`.
/// * `single-alt-pkg` — same identity as `single` but a different `package_dir`
///   (package `other_pkg`), to exercise cross-package bench-name collisions.
///
/// # Panics
///
/// Panics on an unknown preset name.
#[must_use]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn callgrind_preset(kind: &str) -> String {
    match kind {
        "single" => callgrind_summary(
            SINGLE_MODULE_PATH,
            SINGLE_FUNCTION,
            None,
            Some(SINGLE_PACKAGE_DIR),
            36,
            4,
            2,
        ),
        "parametrized" => callgrind_summary(
            PARAMETRIZED_MODULE_PATH,
            PARAMETRIZED_FUNCTION,
            Some("two_instants"),
            Some(SINGLE_PACKAGE_DIR),
            87,
            2,
            1,
        ),
        "single-alt-pkg" => callgrind_summary(
            SINGLE_MODULE_PATH,
            SINGLE_FUNCTION,
            None,
            Some(ALT_PACKAGE_DIR),
            36,
            4,
            2,
        ),
        other => panic!("unknown summary kind {other:?}"),
    }
}

/// Writes a Callgrind `summary.json` under `<target-root>/gungraun/<group…>/`.
///
/// `group` may contain `/`, which is split into nested directory segments — real
/// Gungraun output nests by bench binary then group, so two same-named harnesses in
/// different packages land in distinct nested directories.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn write_callgrind_summary(target_root: &Path, group: &str, content: &str) {
    let mut dir = target_root.join("gungraun");
    for part in group.split('/') {
        dir.push(safe_segment(part));
    }
    std::fs::create_dir_all(&dir).expect("summary directory should be creatable");
    std::fs::write(dir.join("summary.json"), content).expect("summary should be writable");
}

/// Parses a `GROUP|FUNCTION|VALUE=NANOS` argument into a [`CriterionCase`].
///
/// `VALUE` is optional: both `GROUP|FUNCTION=NANOS` and the value-less
/// `GROUP|FUNCTION|=NANOS` form denote a case without a parameter. `GROUP` and
/// `FUNCTION` must be non-empty and no further `|`-separated components are
/// allowed, so a malformed input fails here with a clear message rather than at a
/// later filesystem write.
///
/// # Panics
///
/// Panics on a malformed argument.
#[must_use]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn parse_criterion_arg(value: &str) -> CriterionCase {
    let (identity, nanos) = value
        .split_once('=')
        .expect("--criterion value must be GROUP|FUNCTION|VALUE=NANOS");
    let parts: Vec<&str> = identity.split('|').collect();
    let (group, function, value) = match parts.as_slice() {
        [group, function] => (*group, *function, ""),
        [group, function, value] => (*group, *function, *value),
        _ => panic!(
            "--criterion identity must be GROUP|FUNCTION or GROUP|FUNCTION|VALUE, got {identity:?}"
        ),
    };
    assert!(
        !group.is_empty() && !function.is_empty(),
        "--criterion GROUP and FUNCTION must be non-empty, got {identity:?}"
    );
    CriterionCase {
        group: group.to_owned(),
        function: function.to_owned(),
        value: value.to_owned(),
        nanos: nanos.parse().expect("NANOS must be a number"),
    }
}

/// Writes one Criterion case's `new/benchmark.json` + `new/estimates.json` pair.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn write_criterion_case(target_root: &Path, case: &CriterionCase) {
    // A Criterion group id may legitimately contain `/`; each segment must still be
    // a safe path component before it is flattened into the on-disk directory name.
    for part in case.group.split('/') {
        safe_segment(part);
    }
    let sanitized_group = case.group.replace('/', "_");
    let mut dir = target_root
        .join("criterion")
        .join(sanitized_group)
        .join(safe_segment(&case.function));
    if !case.value.is_empty() {
        dir.push(safe_segment(&case.value));
    }
    dir.push("new");
    std::fs::create_dir_all(&dir).expect("criterion directory should be creatable");

    let full_id = if case.value.is_empty() {
        format!("{}/{}", case.group, case.function)
    } else {
        format!("{}/{}/{}", case.group, case.function, case.value)
    };
    let benchmark = serde_json::json!({
        "group_id": case.group,
        "function_id": case.function,
        "value_str": case.value,
        "throughput": null,
        "full_id": full_id,
        "directory_name": full_id,
        "title": full_id,
    });
    std::fs::write(dir.join("benchmark.json"), benchmark.to_string())
        .expect("benchmark.json should be writable");

    // A minimal but schema-valid estimates document: the slope point estimate is
    // the requested timing, with a narrow confidence interval and small std_dev.
    let nanos = case.nanos;
    let estimate = |point: f64| {
        serde_json::json!({
            "confidence_interval": {
                "confidence_level": 0.95,
                "lower_bound": point - 0.5,
                "upper_bound": point + 0.5,
            },
            "point_estimate": point,
            "standard_error": 0.1,
        })
    };
    let estimates = serde_json::json!({
        "mean": estimate(nanos),
        "median": estimate(nanos),
        "median_abs_dev": estimate(0.2),
        "slope": estimate(nanos),
        "std_dev": estimate(0.3),
    });
    std::fs::write(dir.join("estimates.json"), estimates.to_string())
        .expect("estimates.json should be writable");
}

/// Parses an `OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]` argument into an
/// [`AllocOperation`].
///
/// # Panics
///
/// Panics on a malformed argument.
#[must_use]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn parse_alloc_arg(value: &str) -> AllocOperation {
    let (operation, rest) = value
        .split_once('=')
        .expect("--alloc-tracker value must be OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]");
    assert!(
        !operation.is_empty(),
        "--alloc-tracker OPERATION must be non-empty"
    );
    let (stats, intervals) = match rest.split_once('@') {
        Some((stats, bounds)) => {
            let (bytes_bounds, count_bounds) = bounds
                .split_once('/')
                .expect("--alloc-tracker dispersion must be BLOW:BHIGH/CLOW:CHIGH");
            (
                stats,
                Some((parse_bounds(bytes_bounds), parse_bounds(count_bounds))),
            )
        }
        None => (rest, None),
    };
    let (bytes, count) = stats
        .split_once('/')
        .expect("--alloc-tracker stats must be BYTES/COUNT");
    AllocOperation {
        operation: operation.to_owned(),
        mean_bytes: bytes.parse().expect("BYTES must be a number"),
        mean_allocations: count.parse().expect("COUNT must be a number"),
        intervals,
    }
}

/// Parses an `OPERATION=NANOS[@LOW:HIGH]` argument into a [`TimeOperation`].
///
/// # Panics
///
/// Panics on a malformed argument.
#[must_use]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn parse_time_arg(value: &str) -> TimeOperation {
    let (operation, rest) = value
        .split_once('=')
        .expect("--all-the-time value must be OPERATION=NANOS[@LOW:HIGH]");
    assert!(
        !operation.is_empty(),
        "--all-the-time OPERATION must be non-empty"
    );
    let (nanos, interval) = match rest.split_once('@') {
        Some((nanos, bounds)) => (nanos, Some(parse_bounds(bounds))),
        None => (rest, None),
    };
    TimeOperation {
        operation: operation.to_owned(),
        mean_nanos: nanos.parse().expect("NANOS must be a number"),
        interval,
    }
}

/// Parses a `LOW:HIGH` interval into a `(low, high)` pair. The values are unitless
/// bounds — nanoseconds for `all_the_time`, bytes or allocation counts for
/// `alloc_tracker` — so `LOW` must not exceed `HIGH`, otherwise the emitted
/// dispersion would carry a negative standard deviation and inverted min/max.
#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_bounds(bounds: &str) -> (f64, f64) {
    let (low, high) = bounds.split_once(':').expect("interval must be LOW:HIGH");
    let low: f64 = low.parse().expect("LOW must be a number");
    let high: f64 = high.parse().expect("HIGH must be a number");
    assert!(low <= high, "interval LOW must not exceed HIGH");
    (low, high)
}

/// Writes one `alloc_tracker` `<operation>.json` file under `alloc_tracker/`.
///
/// The shape mirrors the real tool: `total_*` fields derive from the per-iteration
/// slope over a fixed iteration count, and every operation records a span count and
/// per-metric slope. When the request carries per-metric intervals the file
/// additionally records the confidence interval for both the bytes and
/// allocation-count metrics, so the adapter's dispersion path is exercised end to
/// end.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn write_alloc_tracker(target_root: &Path, operation: &AllocOperation) {
    const ITERATIONS: u64 = 4;
    const SPANS: u64 = 6;
    let dir = target_root.join("alloc_tracker");
    std::fs::create_dir_all(&dir).expect("alloc_tracker directory should be creatable");

    let mut output = serde_json::json!({
        "operation": operation.operation,
        "total_iterations": ITERATIONS,
        "total_bytes_allocated": operation.mean_bytes.saturating_mul(ITERATIONS),
        "total_allocations_count": operation.mean_allocations.saturating_mul(ITERATIONS),
        "span_count": if operation.intervals.is_some() { SPANS } else { 1 },
        "slope_bytes_per_iteration": operation.mean_bytes,
        "slope_allocations_per_iteration": operation.mean_allocations,
    });
    if let Some((bytes_interval, count_interval)) = operation.intervals {
        let fields = output
            .as_object_mut()
            .expect("the operation output is a JSON object");
        insert_metric_interval(fields, "bytes_per_iteration", bytes_interval);
        insert_metric_interval(fields, "allocations_per_iteration", count_interval);
    }
    let file = dir.join(format!("{}.json", safe_segment(&operation.operation)));
    std::fs::write(file, output.to_string()).expect("alloc_tracker file should be writable");
}

/// Inserts the confidence-interval bounds for one metric (identified by `suffix`)
/// into an operation output object. Only multi-span output carries an interval; the
/// slope itself is emitted alongside the base fields.
#[cfg_attr(coverage_nightly, coverage(off))]
fn insert_metric_interval(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    suffix: &str,
    interval: (f64, f64),
) {
    let (low, high) = interval;
    fields.insert(format!("interval_low_{suffix}"), serde_json::json!(low));
    fields.insert(format!("interval_high_{suffix}"), serde_json::json!(high));
}

/// Writes one `all_the_time` `<operation>.json` file under `all_the_time/`.
///
/// The shape mirrors the real tool: `total_*` fields derive from the per-iteration
/// slope over a fixed iteration count, and every operation records a span count and
/// slope. When the request carries a confidence interval the file additionally
/// records the interval, so the adapter's dispersion path is exercised end to end.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn write_all_the_time(target_root: &Path, operation: &TimeOperation) {
    const ITERATIONS: u64 = 4;
    const SPANS: u64 = 6;
    let dir = target_root.join("all_the_time");
    std::fs::create_dir_all(&dir).expect("all_the_time directory should be creatable");

    let mut output = serde_json::json!({
        "operation": operation.operation,
        "total_iterations": ITERATIONS,
        "total_processor_time_nanos": operation.mean_nanos.saturating_mul(ITERATIONS),
        "span_count": if operation.interval.is_some() { SPANS } else { 1 },
        "slope_processor_time_nanos": operation.mean_nanos,
    });
    if let Some((low, high)) = operation.interval {
        let fields = output
            .as_object_mut()
            .expect("the operation output is a JSON object");
        insert_metric_interval(fields, "processor_time_nanos", (low, high));
    }
    let file = dir.join(format!("{}.json", safe_segment(&operation.operation)));
    std::fs::write(file, output.to_string()).expect("all_the_time file should be writable");
}

/// Validates that `segment` is safe to use as a single filesystem path component:
/// non-empty, not a current/parent-directory reference, and free of path
/// separators. Inputs are controlled, so misuse is a loud panic rather than a
/// silent escape outside the target root.
#[cfg_attr(coverage_nightly, coverage(off))]
fn safe_segment(segment: &str) -> &str {
    assert!(
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.contains('/')
            && !segment.contains('\\'),
        "unsafe path segment {segment:?}"
    );
    segment
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    #[test]
    fn callgrind_summary_carries_identity_and_tracked_events() {
        let json = callgrind_summary("a::b", "b", Some("case"), Some("/pkg/fast_time"), 36, 4, 2);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["version"], "6");
        assert_eq!(value["module_path"], "a::b");
        assert_eq!(value["function_name"], "b");
        assert_eq!(value["id"], "case");
        assert_eq!(value["package_dir"], "/pkg/fast_time");
        let callgrind = &value["profiles"][0]["summaries"]["total"]["summary"]["Callgrind"];
        assert_eq!(callgrind["Ir"]["metrics"]["Left"]["Int"], 36);
        assert_eq!(callgrind["Bc"]["metrics"]["Left"]["Int"], 4);
        assert_eq!(callgrind["Bi"]["metrics"]["Left"]["Int"], 2);
    }

    #[test]
    fn callgrind_summary_encodes_absent_id_and_package_dir_as_null() {
        let json = callgrind_summary("m", "f", None, None, 1, 0, 0);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(value["id"].is_null());
        assert!(value["package_dir"].is_null());
    }

    #[test]
    fn single_and_alt_presets_differ_only_in_package_dir() {
        let single: serde_json::Value =
            serde_json::from_str(&callgrind_preset("single")).expect("valid JSON");
        let alt: serde_json::Value =
            serde_json::from_str(&callgrind_preset("single-alt-pkg")).expect("valid JSON");
        assert_eq!(single["module_path"], alt["module_path"]);
        assert_eq!(single["function_name"], alt["function_name"]);
        assert_ne!(single["package_dir"], alt["package_dir"]);
        assert_eq!(
            single["package_dir"],
            "/mnt/c/Source/folo/packages/fast_time"
        );
        assert_eq!(alt["package_dir"], "/work/packages/other_pkg");
    }

    #[test]
    fn parametrized_preset_carries_its_id_and_ir() {
        let value: serde_json::Value =
            serde_json::from_str(&callgrind_preset("parametrized")).expect("valid JSON");
        assert_eq!(value["id"], "two_instants");
        let callgrind = &value["profiles"][0]["summaries"]["total"]["summary"]["Callgrind"];
        assert_eq!(callgrind["Ir"]["metrics"]["Left"]["Int"], 87);
    }

    #[test]
    #[should_panic(expected = "unknown summary kind")]
    fn callgrind_preset_rejects_unknown_kind() {
        drop(callgrind_preset("nope"));
    }

    #[test]
    fn parses_a_valueless_criterion_case() {
        let case = parse_criterion_arg("group|func=12.5");
        assert_eq!(case.group, "group");
        assert_eq!(case.function, "func");
        assert!(case.value.is_empty());
        assert!((case.nanos - 12.5).abs() < f64::EPSILON);
    }
}
