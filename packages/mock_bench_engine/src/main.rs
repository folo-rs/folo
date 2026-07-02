//! A stand-in benchmark engine used by the integration tests.
//!
//! It imitates the only parts of a real engine that `cargo bench-history run`
//! observes: it writes machine-readable output files into the cargo target tree
//! (so the harvester finds fresh output) and exits with a caller-chosen code (so
//! exit-code handling can be exercised). It performs no real benchmarking.
//!
//! Usage:
//!
//! ```text
//! mock_bench_engine [--exit-code N] [--summary GROUP=KIND]...
//!                   [--criterion GROUP|FUNCTION|VALUE=NANOS]...
//!                   [--alloc-tracker OPERATION=BYTES/COUNT]...
//!                   [--all-the-time OPERATION=NANOS[@LOW:HIGH]]...
//!                   [--fail-if-exists PATH]
//! ```
//!
//! `--summary GROUP=KIND` writes a Gungraun (Callgrind) `summary.json`, where
//! `KIND` is one of:
//!
//! * `single` — an unparametrized summary, `Ir` = 36, package `fast_time`.
//! * `parametrized` — id `two_instants`, `Ir` = 87, package `fast_time`.
//! * `single-alt-pkg` — identical to `single` (same `module_path`/`function_name`)
//!   but reporting a different `package_dir`, so its `BenchmarkId` differs only in
//!   package. Used to exercise cross-package bench-name collisions.
//!
//! `GROUP` may contain `/`, which is split into nested directory segments under
//! `gungraun/`. Real Gungraun output nests by bench binary (`gungraun/<binary>/
//! <group>/...`), so two same-named bench harnesses in different packages
//! (`foo/benches/a.rs` and `bar/benches/a.rs`) land under the same top-level
//! directory but in distinct nested ones — the on-disk collision shape the
//! recursive harvester must keep distinct.
//!
//! `--criterion GROUP|FUNCTION|VALUE=NANOS` writes a Criterion case as a
//! `new/benchmark.json` and `new/estimates.json` pair whose identity is
//! `GROUP`/`FUNCTION`/`VALUE` (an empty `VALUE` omits the parameter component) and
//! whose wall-clock slope estimate is `NANOS` nanoseconds. The on-disk directory is
//! derived from the identity, so distinct identities never share a directory.
//!
//! `--alloc-tracker OPERATION=BYTES/COUNT` writes an `alloc_tracker`
//! `<OPERATION>.json` file reporting `BYTES` mean bytes and `COUNT` mean
//! allocations per iteration. `--all-the-time OPERATION=NANOS[@LOW:HIGH]` writes
//! an `all_the_time` `<OPERATION>.json` file reporting `NANOS` mean (and slope)
//! processor-time nanoseconds per iteration; an optional `@LOW:HIGH` suffix
//! additionally records a bootstrap confidence interval (and a matching standard
//! deviation, span count and min/max), mirroring the real tool's dispersion
//! output. Both write one flat JSON file per operation under their respective
//! engine directory, mirroring the real tools.
//!
//! `--fail-if-exists PATH` exits with code 1 and writes no output when `PATH`
//! (relative to the working directory) exists. Backfill runs each engine in a
//! checked-out worktree, so a commit that tracks the named marker file stands in
//! for a commit that fails to build or benchmark.
//!
//! `--chdir DIR` changes the working directory to `DIR` before resolving
//! `CARGO_TARGET_DIR` and writing output. This stands in for cargo, which runs a
//! benchmark binary with its working directory set to the owning package's
//! directory — so an engine that honors a relative `CARGO_TARGET_DIR` (such as
//! Criterion) would resolve it there. It lets a test prove the harvest injects an
//! absolute target root that lands output where it scans regardless of that cwd.
//!
//! Summaries are written under `<target-root>`, which honors `CARGO_TARGET_DIR`
//! exactly as the harvester does.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::path::PathBuf;
use std::process::ExitCode;

// mimalloc is a scalable general-purpose allocator: faster small allocations and
// no cross-thread allocator-lock contention (acute on the Windows process heap),
// a broad low-risk win applied uniformly across the workspace's binaries. Miri
// cannot call mimalloc's FFI, so under Miri the default allocator stands in.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// These fixtures live in the sibling `cargo-bench-history` package's test tree
// because they double as schema-drift canaries for that package's parser tests.
// They are intentionally shared (not duplicated) so the mock and the parser can
// never disagree about the on-disk summary shape.
/// A committed Gungraun summary with no `id` (unparametrized), `Ir` = 36.
const SINGLE_SUMMARY: &str = include_str!(
    "../../cargo-bench-history/tests/fixtures/callgrind/single_unparametrized.summary.json"
);
/// A committed Gungraun summary with `id` = `two_instants`, `Ir` = 87.
const PARAMETRIZED_SUMMARY: &str =
    include_str!("../../cargo-bench-history/tests/fixtures/callgrind/parametrized.summary.json");

/// The `package_dir` value of the committed single summary.
const SINGLE_PACKAGE_DIR: &str = "\"/mnt/c/Source/folo/packages/fast_time\"";
/// A different `package_dir` that keeps the `module_path` unchanged, simulating an
/// equally named bench target in another package.
const ALT_PACKAGE_DIR: &str = "\"/work/packages/other_pkg\"";

/// A parsed Criterion case request: its identity and slope estimate (nanoseconds).
struct CriterionCase {
    group: String,
    function: String,
    value: String,
    nanos: f64,
}

/// A parsed `alloc_tracker` operation request: its name and per-iteration means.
struct AllocOperation {
    operation: String,
    mean_bytes: u64,
    mean_allocations: u64,
}

/// A parsed `all_the_time` operation request: its name, per-iteration mean, and
/// an optional bootstrap confidence interval `(low, high)` in nanoseconds. When
/// the interval is present the operation file additionally records a slope,
/// standard deviation, span count and min/max, mirroring dispersion-bearing
/// output.
struct TimeOperation {
    operation: String,
    mean_nanos: u64,
    interval: Option<(f64, f64)>,
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    let mut exit_code: u8 = 0;
    let mut summaries: Vec<(String, String)> = Vec::new();
    let mut criterion_cases: Vec<CriterionCase> = Vec::new();
    let mut alloc_operations: Vec<AllocOperation> = Vec::new();
    let mut time_operations: Vec<TimeOperation> = Vec::new();
    let mut fail_if_exists: Option<PathBuf> = None;
    let mut chdir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-code" => {
                let value = args.next().expect("--exit-code requires a value");
                exit_code = value.parse().expect("--exit-code must be a u8");
            }
            "--summary" => {
                let value = args.next().expect("--summary requires GROUP=KIND");
                let (group, kind) = value
                    .split_once('=')
                    .expect("--summary value must be GROUP=KIND");
                let content = match kind {
                    "single" => SINGLE_SUMMARY.to_owned(),
                    "parametrized" => PARAMETRIZED_SUMMARY.to_owned(),
                    "single-alt-pkg" => SINGLE_SUMMARY.replace(SINGLE_PACKAGE_DIR, ALT_PACKAGE_DIR),
                    other => panic!("unknown summary kind {other:?}"),
                };
                summaries.push((group.to_owned(), content));
            }
            "--criterion" => {
                let value = args
                    .next()
                    .expect("--criterion requires GROUP|FUNCTION|VALUE=NANOS");
                criterion_cases.push(parse_criterion_arg(&value));
            }
            "--alloc-tracker" => {
                let value = args
                    .next()
                    .expect("--alloc-tracker requires OPERATION=BYTES/COUNT");
                alloc_operations.push(parse_alloc_arg(&value));
            }
            "--all-the-time" => {
                let value = args
                    .next()
                    .expect("--all-the-time requires OPERATION=NANOS[@LOW:HIGH]");
                time_operations.push(parse_time_arg(&value));
            }
            "--fail-if-exists" => {
                let value = args.next().expect("--fail-if-exists requires a PATH");
                fail_if_exists = Some(PathBuf::from(value));
            }
            "--chdir" => {
                let value = args.next().expect("--chdir requires a DIR");
                chdir = Some(PathBuf::from(value));
            }
            // The production flow appends cargo scope flags (`--workspace`,
            // `--package`, `--bench`) and `--` passthrough after the mock's own
            // contiguous arguments; stop once those begin.
            _ => break,
        }
    }

    // Stand in for a commit that fails to build: when the marker is present in the
    // checked-out worktree, exit non-zero before writing any output.
    if let Some(marker) = &fail_if_exists
        && marker.exists()
    {
        return ExitCode::from(1);
    }

    // Stand in for cargo running a benchmark binary with its working directory set
    // to the owning package's directory: change into it before a possibly relative
    // CARGO_TARGET_DIR is resolved, so output lands wherever that resolution points.
    if let Some(dir) = &chdir {
        std::env::set_current_dir(dir).expect("--chdir directory should exist");
    }

    let target_root =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);

    for (group, content) in &summaries {
        // A group may name nested directories (a real engine nests by bench binary
        // then group), so split on `/` and validate each component separately.
        let mut dir = target_root.join("gungraun");
        for part in group.split('/') {
            dir.push(safe_segment(part));
        }
        std::fs::create_dir_all(&dir).expect("summary directory should be creatable");
        std::fs::write(dir.join("summary.json"), content).expect("summary should be writable");
    }

    for case in &criterion_cases {
        write_criterion_case(&target_root, case);
    }

    for operation in &alloc_operations {
        write_alloc_operation(&target_root, operation);
    }

    for operation in &time_operations {
        write_time_operation(&target_root, operation);
    }

    ExitCode::from(exit_code)
}

/// Parses a `GROUP|FUNCTION|VALUE=NANOS` argument into a [`CriterionCase`].
///
/// `VALUE` is optional: both `GROUP|FUNCTION=NANOS` and the value-less
/// `GROUP|FUNCTION|=NANOS` form denote a case without a parameter. `GROUP` and
/// `FUNCTION` must be non-empty and no further `|`-separated components are
/// allowed, so a malformed fixture fails here with a clear message rather than at
/// a later filesystem write.
#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_criterion_arg(value: &str) -> CriterionCase {
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
fn write_criterion_case(target_root: &std::path::Path, case: &CriterionCase) {
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

/// Parses an `OPERATION=BYTES/COUNT` argument into an [`AllocOperation`].
#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_alloc_arg(value: &str) -> AllocOperation {
    let (operation, stats) = value
        .split_once('=')
        .expect("--alloc-tracker value must be OPERATION=BYTES/COUNT");
    let (bytes, count) = stats
        .split_once('/')
        .expect("--alloc-tracker stats must be BYTES/COUNT");
    assert!(
        !operation.is_empty(),
        "--alloc-tracker OPERATION must be non-empty"
    );
    AllocOperation {
        operation: operation.to_owned(),
        mean_bytes: bytes.parse().expect("BYTES must be a number"),
        mean_allocations: count.parse().expect("COUNT must be a number"),
    }
}

/// Parses an `OPERATION=NANOS[@LOW:HIGH]` argument into a [`TimeOperation`].
#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_time_arg(value: &str) -> TimeOperation {
    let (operation, rest) = value
        .split_once('=')
        .expect("--all-the-time value must be OPERATION=NANOS[@LOW:HIGH]");
    assert!(
        !operation.is_empty(),
        "--all-the-time OPERATION must be non-empty"
    );
    let (nanos, interval) = match rest.split_once('@') {
        Some((nanos, bounds)) => {
            let (low, high) = bounds
                .split_once(':')
                .expect("--all-the-time interval must be LOW:HIGH");
            (
                nanos,
                Some((
                    low.parse().expect("LOW must be a number"),
                    high.parse().expect("HIGH must be a number"),
                )),
            )
        }
        None => (rest, None),
    };
    TimeOperation {
        operation: operation.to_owned(),
        mean_nanos: nanos.parse().expect("NANOS must be a number"),
        interval,
    }
}

/// Writes one `alloc_tracker` `<operation>.json` file under `alloc_tracker/`.
///
/// The shape mirrors the real tool: `total_*` fields derive from the per-iteration
/// means over a fixed iteration count, though the harvest reads only the means.
#[cfg_attr(coverage_nightly, coverage(off))]
fn write_alloc_operation(target_root: &std::path::Path, operation: &AllocOperation) {
    const ITERATIONS: u64 = 4;
    let dir = target_root.join("alloc_tracker");
    std::fs::create_dir_all(&dir).expect("alloc_tracker directory should be creatable");

    let output = serde_json::json!({
        "operation": operation.operation,
        "total_iterations": ITERATIONS,
        "total_bytes_allocated": operation.mean_bytes.saturating_mul(ITERATIONS),
        "total_allocations_count": operation.mean_allocations.saturating_mul(ITERATIONS),
        "mean_bytes_per_iteration": operation.mean_bytes,
        "mean_allocations_per_iteration": operation.mean_allocations,
    });
    let file = dir.join(format!("{}.json", safe_segment(&operation.operation)));
    std::fs::write(file, output.to_string()).expect("alloc_tracker file should be writable");
}

/// Writes one `all_the_time` `<operation>.json` file under `all_the_time/`.
///
/// The shape mirrors the real tool: `total_*` fields derive from the per-iteration
/// mean over a fixed iteration count. When the request carries a confidence
/// interval the file additionally records the slope, standard deviation, span
/// count and min/max, so the adapter's dispersion path is exercised end to end.
#[cfg_attr(coverage_nightly, coverage(off))]
fn write_time_operation(target_root: &std::path::Path, operation: &TimeOperation) {
    const ITERATIONS: u64 = 4;
    const SPANS: u64 = 6;
    let dir = target_root.join("all_the_time");
    std::fs::create_dir_all(&dir).expect("all_the_time directory should be creatable");

    let mut output = serde_json::json!({
        "operation": operation.operation,
        "total_iterations": ITERATIONS,
        "total_processor_time_nanos": operation.mean_nanos.saturating_mul(ITERATIONS),
        "mean_processor_time_nanos": operation.mean_nanos,
    });
    if let Some((low, high)) = operation.interval {
        let std_dev = (high - low) / 2.0;
        let fields = output
            .as_object_mut()
            .expect("the operation output is a JSON object");
        // The slope is emitted as the integer mean, which deserializes to the
        // adapter's `f64` slope; the interval bounds and standard deviation
        // carry the dispersion the noise detector reads.
        fields.insert("span_count".to_owned(), serde_json::json!(SPANS));
        fields.insert(
            "slope_processor_time_nanos".to_owned(),
            serde_json::json!(operation.mean_nanos),
        );
        fields.insert(
            "std_dev_processor_time_nanos".to_owned(),
            serde_json::json!(std_dev),
        );
        fields.insert(
            "interval_low_processor_time_nanos".to_owned(),
            serde_json::json!(low),
        );
        fields.insert(
            "interval_high_processor_time_nanos".to_owned(),
            serde_json::json!(high),
        );
        fields.insert(
            "min_processor_time_nanos".to_owned(),
            serde_json::json!(low),
        );
        fields.insert(
            "max_processor_time_nanos".to_owned(),
            serde_json::json!(high),
        );
    }
    let file = dir.join(format!("{}.json", safe_segment(&operation.operation)));
    std::fs::write(file, output.to_string()).expect("all_the_time file should be writable");
}

/// Validates that `segment` is safe to use as a single filesystem path component:
/// non-empty, not a current/parent-directory reference, and free of path
/// separators. Test inputs are controlled, so misuse is a loud panic rather than a
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
