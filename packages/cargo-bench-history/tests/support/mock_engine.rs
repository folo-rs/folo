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
            other => panic!("unknown argument {other:?}"),
        }
    }

    let target_root =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);

    for (group, content) in &summaries {
        let dir = target_root.join("gungraun").join(group);
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
    let sanitized_group = case.group.replace('/', "_");
    let mut dir = target_root
        .join("criterion")
        .join(sanitized_group)
        .join(&case.function);
    if !case.value.is_empty() {
        dir.push(&case.value);
    }
    dir.push("new");
    std::fs::create_dir_all(&dir).expect("criterion directory should be creatable");

    let full_id = if case.value.is_empty() {
        format!("{}/{}", case.group, case.function)
    } else {
        format!("{}/{}/{}", case.group, case.function, case.value)
    };
    let benchmark = format!(
        "{{\"group_id\":\"{}\",\"function_id\":\"{}\",\"value_str\":\"{}\",\
         \"throughput\":null,\"full_id\":\"{full_id}\",\
         \"directory_name\":\"{full_id}\",\"title\":\"{full_id}\"}}",
        case.group, case.function, case.value,
    );
    std::fs::write(dir.join("benchmark.json"), benchmark)
        .expect("benchmark.json should be writable");

    // A minimal but schema-valid estimates document: the slope point estimate is
    // the requested timing, with a narrow confidence interval and small std_dev.
    let nanos = case.nanos;
    let low = nanos - 0.5;
    let high = nanos + 0.5;
    let estimate = |point: f64| {
        format!(
            "{{\"confidence_interval\":{{\"confidence_level\":0.95,\
             \"lower_bound\":{:.4},\"upper_bound\":{:.4}}},\
             \"point_estimate\":{point},\"standard_error\":0.1}}",
            point - 0.5,
            point + 0.5,
        )
    };
    let slope = format!(
        "{{\"confidence_interval\":{{\"confidence_level\":0.95,\
         \"lower_bound\":{low:.4},\"upper_bound\":{high:.4}}},\
         \"point_estimate\":{nanos},\"standard_error\":0.1}}"
    );
    let estimates = format!(
        "{{\"mean\":{},\"median\":{},\"median_abs_dev\":{},\"slope\":{slope},\"std_dev\":{}}}",
        estimate(nanos),
        estimate(nanos),
        estimate(0.2),
        estimate(0.3),
    );
    std::fs::write(dir.join("estimates.json"), estimates)
        .expect("estimates.json should be writable");
}
