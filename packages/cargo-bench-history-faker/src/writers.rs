//! The synthetic benchmark-output writers and their pure value cores.
//!
//! Each engine has a pure value core that builds the on-disk document as a value
//! (constructed without touching the filesystem, so it is usable from unit tests
//! and Miri) and a thin `write_*` wrapper that serializes it into the cargo target
//! tree exactly where the real engine would, honoring `CARGO_TARGET_DIR`.
//!
//! None of this is a stable API — see the crate root.

use std::path::Path;

/// A parsed Callgrind case request: its on-disk group, identity, and the three
/// tracked instruction/branch counts.
#[derive(Debug)]
#[non_exhaustive]
pub struct CallgrindCase {
    /// The on-disk group path under `gungraun/` (may contain `/`).
    pub group: String,
    /// The `module_path` recorded in the summary's identity.
    pub module_path: String,
    /// The `function_name` recorded in the summary's identity.
    pub function: String,
    /// The optional parameter id (`None` for an unparametrized case).
    pub id: Option<String>,
    /// The optional `package_dir`; its final component becomes the package segment
    /// of the parsed `BenchmarkId`.
    pub package_dir: Option<String>,
    /// The `Ir` (instructions retired) count.
    pub ir: u64,
    /// The `Bc` (conditional branches) count.
    pub bc: u64,
    /// The `Bi` (indirect branches) count.
    pub bi: u64,
}

/// A parsed Criterion case request: its identity, slope estimate (nanoseconds),
/// and optional dispersion.
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
    /// Optional dispersion `(std_dev, (ci_low, ci_high))`. When present the case
    /// records that standard deviation and confidence interval on its estimates;
    /// when absent it emits a degenerate `(nanos, nanos)` interval and no standard
    /// deviation, so no dispersion the caller did not request is invented.
    pub dispersion: Option<(f64, (f64, f64))>,
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

/// Parses a `GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]]=IR/BC/BI` argument into a
/// [`CallgrindCase`].
///
/// `GROUP` is the on-disk directory (may contain `/`); `MODULE` and `FUNCTION`
/// form the identity and must be non-empty. `ID` and `PACKAGE_DIR` are optional
/// (an empty or omitted component denotes absence). `IR`/`BC`/`BI` are the three
/// tracked counts.
///
/// # Panics
///
/// Panics on a malformed argument.
#[must_use]
pub fn parse_callgrind_arg(value: &str) -> CallgrindCase {
    let (identity, metrics) = value
        .split_once('=')
        .expect("--callgrind value must be GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]]=IR/BC/BI");
    let parts: Vec<&str> = identity.split('|').collect();
    let (group, module_path, function, id, package_dir) = match parts.as_slice() {
        [group, module, function] => (*group, *module, *function, "", ""),
        [group, module, function, id] => (*group, *module, *function, *id, ""),
        [group, module, function, id, package] => (*group, *module, *function, *id, *package),
        _ => panic!(
            "--callgrind identity must be GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]], got {identity:?}"
        ),
    };
    assert!(
        !group.is_empty() && !module_path.is_empty() && !function.is_empty(),
        "--callgrind GROUP, MODULE and FUNCTION must be non-empty, got {identity:?}"
    );
    let counts: Vec<&str> = metrics.split('/').collect();
    let [ir, bc, bi] = counts.as_slice() else {
        panic!("--callgrind metrics must be IR/BC/BI, got {metrics:?}");
    };
    CallgrindCase {
        group: group.to_owned(),
        module_path: module_path.to_owned(),
        function: function.to_owned(),
        id: (!id.is_empty()).then(|| id.to_owned()),
        package_dir: (!package_dir.is_empty()).then(|| package_dir.to_owned()),
        ir: ir.parse().expect("IR must be a number"),
        bc: bc.parse().expect("BC must be a number"),
        bi: bi.parse().expect("BI must be a number"),
    }
}

/// Writes a Callgrind `summary.json` under `<target-root>/gungraun/<group…>/`.
///
/// `group` may contain `/`, which is split into nested directory segments — real
/// Gungraun output nests by bench binary then group, so two same-named harnesses in
/// different packages land in distinct nested directories.
pub fn write_callgrind_summary(target_root: &Path, group: &str, content: &str) {
    let mut dir = target_root.join("gungraun");
    for part in group.split('/') {
        dir.push(safe_segment(part));
    }
    std::fs::create_dir_all(&dir).expect("summary directory should be creatable");
    std::fs::write(dir.join("summary.json"), content).expect("summary should be writable");
}

/// Builds a [`CallgrindCase`]'s summary document and writes it under its group.
///
/// The thin case-to-filesystem wrapper the binary drives: it serializes the case
/// through [`callgrind_summary`] and deposits it via [`write_callgrind_summary`],
/// so every emitted identity and count originates from the parsed case.
pub fn write_callgrind_case(target_root: &Path, case: &CallgrindCase) {
    let content = callgrind_summary(
        &case.module_path,
        &case.function,
        case.id.as_deref(),
        case.package_dir.as_deref(),
        case.ir,
        case.bc,
        case.bi,
    );
    write_callgrind_summary(target_root, &case.group, &content);
}

/// Parses a `GROUP|FUNCTION|VALUE=NANOS[@STDDEV/CILOW:CIHIGH]` argument into a
/// [`CriterionCase`].
///
/// `VALUE` is optional: both `GROUP|FUNCTION=NANOS` and the value-less
/// `GROUP|FUNCTION|=NANOS` form denote a case without a parameter. `GROUP` and
/// `FUNCTION` must be non-empty and no further `|`-separated components are
/// allowed, so a malformed input fails here with a clear message rather than at a
/// later filesystem write.
///
/// The optional `@STDDEV/CILOW:CIHIGH` suffix records dispersion: `STDDEV` is the
/// standard deviation and `CILOW:CIHIGH` the confidence interval on the estimates.
/// When omitted, the case carries no dispersion and later emits a degenerate
/// interval with no standard deviation.
///
/// # Panics
///
/// Panics on a malformed argument.
#[must_use]
pub fn parse_criterion_arg(value: &str) -> CriterionCase {
    let (identity, timing) = value
        .split_once('=')
        .expect("--criterion value must be GROUP|FUNCTION|VALUE=NANOS[@STDDEV/CILOW:CIHIGH]");
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
    let (nanos, dispersion) = match timing.split_once('@') {
        Some((nanos, suffix)) => {
            let (std_dev, ci) = suffix
                .split_once('/')
                .expect("--criterion dispersion must be STDDEV/CILOW:CIHIGH");
            (
                nanos,
                Some((
                    std_dev.parse::<f64>().expect("STDDEV must be a number"),
                    parse_bounds(ci),
                )),
            )
        }
        None => (timing, None),
    };
    CriterionCase {
        group: group.to_owned(),
        function: function.to_owned(),
        value: value.to_owned(),
        nanos: nanos.parse().expect("NANOS must be a number"),
        dispersion,
    }
}

/// Writes one Criterion case's `new/benchmark.json` + `new/estimates.json` pair.
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

    // A minimal, schema-valid estimates document driven entirely by the case. The
    // parser prefers `slope` and reads only the confidence interval bounds and
    // `std_dev.point_estimate`, so nothing beyond what the case describes is
    // emitted: without a dispersion request the interval is degenerate
    // `(nanos, nanos)` and no `std_dev` is written, so no dispersion is invented.
    let nanos = case.nanos;
    let (ci_low, ci_high) = case.dispersion.map_or((nanos, nanos), |(_, ci)| ci);
    let estimate = |point: f64, low: f64, high: f64| {
        serde_json::json!({
            "confidence_interval": {
                "lower_bound": low,
                "upper_bound": high,
            },
            "point_estimate": point,
        })
    };
    let mut estimates = serde_json::json!({
        "mean": estimate(nanos, ci_low, ci_high),
        "slope": estimate(nanos, ci_low, ci_high),
    });
    if let Some((std_dev, _)) = case.dispersion {
        estimates
            .as_object_mut()
            .expect("the estimates document is a JSON object")
            .insert("std_dev".to_owned(), estimate(std_dev, std_dev, std_dev));
    }
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
    fn parses_a_full_callgrind_case() {
        let case = parse_callgrind_arg("shared/foo|a::b::c|c|two_instants|/pkg/fast_time=36/4/2");
        assert_eq!(case.group, "shared/foo");
        assert_eq!(case.module_path, "a::b::c");
        assert_eq!(case.function, "c");
        assert_eq!(case.id.as_deref(), Some("two_instants"));
        assert_eq!(case.package_dir.as_deref(), Some("/pkg/fast_time"));
        assert_eq!(case.ir, 36);
        assert_eq!(case.bc, 4);
        assert_eq!(case.bi, 2);
    }

    #[test]
    fn parses_a_minimal_callgrind_case_with_absent_optionals() {
        let case = parse_callgrind_arg("grp|m|f=1/0/0");
        assert!(case.id.is_none());
        assert!(case.package_dir.is_none());
        assert_eq!(case.ir, 1);
    }

    #[test]
    fn parses_a_callgrind_case_with_empty_optionals_as_absent() {
        let case = parse_callgrind_arg("grp|m|f||=5/1/1");
        assert!(case.id.is_none());
        assert!(case.package_dir.is_none());
    }

    #[test]
    #[should_panic(expected = "--callgrind metrics must be IR/BC/BI")]
    fn callgrind_arg_rejects_missing_metrics() {
        drop(parse_callgrind_arg("grp|m|f=36/4"));
    }

    #[test]
    fn parses_a_valueless_criterion_case_without_dispersion() {
        let case = parse_criterion_arg("group|func=12.5");
        assert_eq!(case.group, "group");
        assert_eq!(case.function, "func");
        assert!(case.value.is_empty());
        assert!((case.nanos - 12.5).abs() < f64::EPSILON);
        assert!(case.dispersion.is_none());
    }

    #[test]
    fn parses_a_criterion_case_with_dispersion() {
        let case = parse_criterion_arg("group|func|val=26.9@0.5/26.4:27.4");
        assert_eq!(case.value, "val");
        let (std_dev, (low, high)) = case.dispersion.expect("dispersion parsed");
        assert!((std_dev - 0.5).abs() < f64::EPSILON);
        assert!((low - 26.4).abs() < f64::EPSILON);
        assert!((high - 27.4).abs() < f64::EPSILON);
    }
}
