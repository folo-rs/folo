//! The synthetic benchmark-output writers and their pure value cores.
//!
//! Each engine has a pure value core that builds the on-disk document as a value
//! (constructed without touching the filesystem, so it is usable from unit tests
//! and Miri) and a thin `write_*` wrapper that serializes it into the cargo target
//! tree exactly where the real engine would, honoring `CARGO_TARGET_DIR`.
//!
//! None of this is a stable API — see the crate root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use folo_utils::sanitize_file_name;

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
/// intervals `((bytes_low, bytes_high), (count_low, count_high))`.
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
/// `(low, high)` in nanoseconds.
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
    let dir = callgrind_group_dir(target_root, group);
    std::fs::create_dir_all(&dir).expect("summary directory should be creatable");
    std::fs::write(dir.join("summary.json"), content).expect("summary should be writable");
}

/// Returns the sanitized directory for one Callgrind group.
fn callgrind_group_dir(target_root: &Path, group: &str) -> PathBuf {
    let mut dir = target_root.join("gungraun");
    for part in group.split('/') {
        dir.push(sanitize_file_name(part));
    }
    dir
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

/// Writes Callgrind cases after rejecting colliding group destinations.
///
/// # Panics
///
/// Panics if output cannot be written or two groups sanitize to the same summary
/// path.
pub fn write_callgrind_cases(target_root: &Path, cases: &[CallgrindCase]) {
    assert_unique_output_paths(cases.iter().map(|case| {
        (
            case.group.clone(),
            callgrind_group_dir(target_root, &case.group).join("summary.json"),
        )
    }));
    for case in cases {
        write_callgrind_case(target_root, case);
    }
}

/// Parses a `GROUP|FUNCTION[|VALUE]=NANOS[@STDDEV/CILOW:CIHIGH]` argument into a
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
        .expect("--criterion value must be GROUP|FUNCTION[|VALUE]=NANOS[@STDDEV/CILOW:CIHIGH]");
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

/// Returns the sanitized `new/` directory for one Criterion case.
fn criterion_case_dir(target_root: &Path, case: &CriterionCase) -> PathBuf {
    let mut dir = target_root
        .join("criterion")
        .join(sanitize_file_name(&case.group))
        .join(sanitize_file_name(&case.function));
    if !case.value.is_empty() {
        dir.push(sanitize_file_name(&case.value));
    }
    dir.push("new");
    dir
}

/// Returns a Criterion case's unsanitized identity.
fn criterion_full_id(case: &CriterionCase) -> String {
    if case.value.is_empty() {
        format!("{}/{}", case.group, case.function)
    } else {
        format!("{}/{}/{}", case.group, case.function, case.value)
    }
}

/// Writes one Criterion case's `new/benchmark.json` + `new/estimates.json` pair.
pub fn write_criterion_case(target_root: &Path, case: &CriterionCase) {
    let dir = criterion_case_dir(target_root, case);
    std::fs::create_dir_all(&dir).expect("criterion directory should be creatable");

    let full_id = criterion_full_id(case);
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

/// Writes Criterion cases after rejecting colliding sanitized destinations.
///
/// # Panics
///
/// Panics if output cannot be written or two cases sanitize to the same output
/// directory.
pub fn write_criterion_cases(target_root: &Path, cases: &[CriterionCase]) {
    assert_unique_output_paths(cases.iter().map(|case| {
        (
            criterion_full_id(case),
            criterion_case_dir(target_root, case).join("benchmark.json"),
        )
    }));
    for case in cases {
        write_criterion_case(target_root, case);
    }
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

/// Builds the parser-visible fields for one `alloc_tracker` operation.
fn alloc_tracker_output(operation: &AllocOperation) -> serde_json::Value {
    let mut output = serde_json::json!({
        "operation": operation.operation,
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
    output
}

/// Writes `alloc_tracker` operation files under `alloc_tracker/`.
///
/// Only fields read by `cargo-bench-history` are emitted. File names use the same
/// sanitizer as the real producer while the JSON keeps the original operation
/// identity.
///
/// # Panics
///
/// Panics if output cannot be written or two operation names sanitize to the same
/// file name.
pub fn write_alloc_tracker(target_root: &Path, operations: &[AllocOperation]) {
    let outputs = operation_outputs(operations.iter().map(|operation| {
        (
            operation.operation.as_str(),
            alloc_tracker_output(operation),
        )
    }));
    write_operation_outputs(target_root, "alloc_tracker", outputs);
}

/// Inserts the requested confidence-interval bounds for one operation metric.
fn insert_metric_interval(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    suffix: &str,
    interval: (f64, f64),
) {
    let (low, high) = interval;
    fields.insert(format!("interval_low_{suffix}"), serde_json::json!(low));
    fields.insert(format!("interval_high_{suffix}"), serde_json::json!(high));
}

/// Builds the parser-visible fields for one `all_the_time` operation.
fn all_the_time_output(operation: &TimeOperation) -> serde_json::Value {
    let mut output = serde_json::json!({
        "operation": operation.operation,
        "slope_processor_time_nanos": operation.mean_nanos,
    });
    if let Some((low, high)) = operation.interval {
        let fields = output
            .as_object_mut()
            .expect("the operation output is a JSON object");
        insert_metric_interval(fields, "processor_time_nanos", (low, high));
    }
    output
}

/// Writes `all_the_time` operation files under `all_the_time/`.
///
/// Only fields read by `cargo-bench-history` are emitted. File names use the same
/// sanitizer as the real producer while the JSON keeps the original operation
/// identity.
///
/// # Panics
///
/// Panics if output cannot be written or two operation names sanitize to the same
/// file name.
pub fn write_all_the_time(target_root: &Path, operations: &[TimeOperation]) {
    let outputs = operation_outputs(
        operations
            .iter()
            .map(|operation| (operation.operation.as_str(), all_the_time_output(operation))),
    );
    write_operation_outputs(target_root, "all_the_time", outputs);
}

/// Sanitizes operation file names and rejects collisions before touching disk.
fn operation_outputs<'a>(
    outputs: impl IntoIterator<Item = (&'a str, serde_json::Value)>,
) -> Vec<(String, serde_json::Value)> {
    let mut file_names: HashMap<String, String> = HashMap::new();

    outputs
        .into_iter()
        .map(|(operation, output)| {
            let file_name = format!("{}.json", sanitize_file_name(operation));
            let collision_key = file_name.to_ascii_lowercase();
            if let Some(previous) = file_names.insert(collision_key, operation.to_owned()) {
                panic!(
                    "operations {previous:?} and {operation:?} both map to the output file name \
                     {file_name:?} after sanitization"
                );
            }
            (file_name, output)
        })
        .collect()
}

/// Rejects output paths that collide on a case-insensitive filesystem.
fn assert_unique_output_paths(outputs: impl IntoIterator<Item = (String, PathBuf)>) {
    let mut paths: Vec<(Vec<String>, String)> = Vec::new();

    for (identity, path) in outputs {
        let collision_key: Vec<String> = path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect();
        if let Some((_, previous)) = paths.iter().find(|(existing, _)| {
            existing.starts_with(&collision_key) || collision_key.starts_with(existing)
        }) {
            panic!(
                "cases {previous:?} and {identity:?} map to conflicting output paths after \
                 sanitization; one output file is equal to or nested under the other ({path:?})"
            );
        }
        paths.push((collision_key, identity));
    }
}

/// Writes prepared operation documents under one engine's target subdirectory.
fn write_operation_outputs(
    target_root: &Path,
    engine: &str,
    outputs: Vec<(String, serde_json::Value)>,
) {
    if outputs.is_empty() {
        return;
    }

    let dir = target_root.join(engine);
    std::fs::create_dir_all(&dir).expect("the operation output directory should be creatable");
    for (file_name, output) in outputs {
        std::fs::write(dir.join(file_name), output.to_string())
            .expect("the operation output file should be writable");
    }
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

    #[test]
    fn alloc_tracker_output_contains_only_requested_values() {
        let operation = parse_alloc_arg("group/case=200/3@190:210/2:4");
        assert_eq!(
            alloc_tracker_output(&operation),
            serde_json::json!({
                "operation": "group/case",
                "slope_bytes_per_iteration": 200,
                "slope_allocations_per_iteration": 3,
                "interval_low_bytes_per_iteration": 190.0,
                "interval_high_bytes_per_iteration": 210.0,
                "interval_low_allocations_per_iteration": 2.0,
                "interval_high_allocations_per_iteration": 4.0,
            })
        );
    }

    #[test]
    fn all_the_time_output_contains_only_requested_values() {
        let operation = parse_time_arg("group/case=25@20:30");
        assert_eq!(
            all_the_time_output(&operation),
            serde_json::json!({
                "operation": "group/case",
                "slope_processor_time_nanos": 25,
                "interval_low_processor_time_nanos": 20.0,
                "interval_high_processor_time_nanos": 30.0,
            })
        );
    }

    #[test]
    fn operation_file_names_match_the_real_producer_sanitization() {
        let outputs = operation_outputs([("group/case", serde_json::json!({}))]);
        assert_eq!(outputs[0].0, "group_case.json");
    }

    #[test]
    #[should_panic]
    fn operation_file_name_collisions_are_rejected() {
        drop(operation_outputs([
            ("group/case", serde_json::json!({})),
            ("group_case", serde_json::json!({})),
        ]));
    }

    #[test]
    #[should_panic]
    fn operation_file_name_collisions_ignore_ascii_case() {
        drop(operation_outputs([
            ("Group", serde_json::json!({})),
            ("group", serde_json::json!({})),
        ]));
    }

    #[test]
    #[should_panic]
    fn callgrind_group_collisions_are_rejected_before_writing() {
        let cases = [
            parse_callgrind_arg("a:b|m|f=1/0/0"),
            parse_callgrind_arg("a?b|m|f=2/0/0"),
        ];
        write_callgrind_cases(Path::new("unused"), &cases);
    }

    #[test]
    #[should_panic]
    fn callgrind_file_and_directory_collisions_are_rejected_before_writing() {
        let cases = [
            parse_callgrind_arg("a|m|f=1/0/0"),
            parse_callgrind_arg("a/summary.json/b|m|f=2/0/0"),
        ];
        write_callgrind_cases(Path::new("unused"), &cases);
    }

    #[test]
    #[should_panic]
    fn criterion_case_collisions_are_rejected_before_writing() {
        let cases = [
            parse_criterion_arg("a:b|f=1"),
            parse_criterion_arg("a?b|f=2"),
        ];
        write_criterion_cases(Path::new("unused"), &cases);
    }
}
