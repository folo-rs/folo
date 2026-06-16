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
//! cargo-bench-history-mock-engine [--exit-code N] [--summary GROUP=KIND]...
//!                                 [--criterion GROUP|FUNCTION|VALUE=NANOS]...
//!                                 [--fail-if-exists PATH]
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
//! `--criterion GROUP|FUNCTION|VALUE=NANOS` writes a Criterion case as a
//! `new/benchmark.json` and `new/estimates.json` pair whose identity is
//! `GROUP`/`FUNCTION`/`VALUE` (an empty `VALUE` omits the parameter component) and
//! whose wall-clock slope estimate is `NANOS` nanoseconds. The on-disk directory is
//! derived from the identity, so distinct identities never share a directory.
//!
//! `--fail-if-exists PATH` exits with code 1 and writes no output when `PATH`
//! (relative to the working directory) exists. Backfill runs each engine in a
//! checked-out worktree, so a commit that tracks the named marker file stands in
//! for a commit that fails to build or benchmark.
//!
//! Summaries are written under `<target-root>`, which honors `CARGO_TARGET_DIR`
//! exactly as the harvester does.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::path::PathBuf;
use std::process::ExitCode;

/// A committed Gungraun summary with no `id` (unparametrized), `Ir` = 36.
const SINGLE_SUMMARY: &str =
    include_str!("../fixtures/callgrind/single_unparametrized.summary.json");
/// A committed Gungraun summary with `id` = `two_instants`, `Ir` = 87.
const PARAMETRIZED_SUMMARY: &str = include_str!("../fixtures/callgrind/parametrized.summary.json");

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

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    let mut exit_code: u8 = 0;
    let mut summaries: Vec<(String, String)> = Vec::new();
    let mut criterion_cases: Vec<CriterionCase> = Vec::new();
    let mut fail_if_exists: Option<PathBuf> = None;

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
            "--fail-if-exists" => {
                let value = args.next().expect("--fail-if-exists requires a PATH");
                fail_if_exists = Some(PathBuf::from(value));
            }
            other => panic!("unknown argument {other:?}"),
        }
    }

    // Stand in for a commit that fails to build: when the marker is present in the
    // checked-out worktree, exit non-zero before writing any output.
    if let Some(marker) = &fail_if_exists
        && marker.exists()
    {
        return ExitCode::from(1);
    }

    let target_root =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);

    for (group, content) in &summaries {
        let dir = target_root.join("gungraun").join(safe_segment(group));
        std::fs::create_dir_all(&dir).expect("summary directory should be creatable");
        std::fs::write(dir.join("summary.json"), content).expect("summary should be writable");
    }

    for case in &criterion_cases {
        write_criterion_case(&target_root, case);
    }

    ExitCode::from(exit_code)
}

/// Parses a `GROUP|FUNCTION|VALUE=NANOS` argument into a [`CriterionCase`].
#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_criterion_arg(value: &str) -> CriterionCase {
    let (identity, nanos) = value
        .split_once('=')
        .expect("--criterion value must be GROUP|FUNCTION|VALUE=NANOS");
    let mut parts = identity.split('|');
    let group = parts.next().expect("missing GROUP").to_owned();
    let function = parts.next().expect("missing FUNCTION").to_owned();
    let value = parts.next().unwrap_or("").to_owned();
    CriterionCase {
        group,
        function,
        value,
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
