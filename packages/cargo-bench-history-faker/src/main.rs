//! The `cargo-bench-history-faker` binary: a stand-in benchmark engine.
//!
//! It imitates the only parts of a real engine that `cargo-bench-history`
//! observes: it writes machine-readable output files into the cargo target tree
//! (so a harvest finds fresh output) and exits with a caller-chosen code (so
//! exit-code handling can be exercised). It performs no real benchmarking. The
//! command line is **unsupported** and may change without notice — see the crate
//! documentation.
//!
//! Usage:
//!
//! ```text
//! cargo-bench-history-faker [--exit-code N]
//!                           [--callgrind GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]]=IR/BC/BI]...
//!                           [--criterion GROUP|FUNCTION|VALUE=NANOS[@STDDEV/CILOW:CIHIGH]]...
//!                           [--alloc-tracker OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]]...
//!                           [--all-the-time OPERATION=NANOS[@LOW:HIGH]]...
//!                           [--fail-if-exists PATH] [--chdir DIR]
//! ```
//!
//! `--callgrind GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]]=IR/BC/BI` writes a Gungraun
//! (Callgrind) `summary.json` whose every field is taken verbatim from the
//! argument. The recorded identity is `MODULE`/`FUNCTION` with the optional `ID`
//! parameter and `PACKAGE_DIR` (whose final path component becomes the parsed
//! package name); an empty or omitted `ID`/`PACKAGE_DIR` records its absence. The
//! tracked counts are `IR` (instructions retired), `BC` (conditional branches) and
//! `BI` (indirect branches). `GROUP` is the on-disk directory and may contain `/`,
//! which is split into nested segments under `gungraun/`. Real Gungraun output nests
//! by bench binary (`gungraun/<binary>/<group>/...`), so two same-named bench
//! harnesses in different packages share a top-level directory but occupy distinct
//! nested ones — the on-disk collision shape a recursive harvest must keep distinct.
//!
//! `--criterion GROUP|FUNCTION|VALUE=NANOS[@STDDEV/CILOW:CIHIGH]` writes a Criterion
//! case as a `new/benchmark.json` and `new/estimates.json` pair whose identity is
//! `GROUP`/`FUNCTION`/`VALUE` (an empty `VALUE` omits the parameter component) and
//! whose wall-clock slope estimate is `NANOS` nanoseconds. The optional
//! `@STDDEV/CILOW:CIHIGH` suffix records a standard deviation and confidence
//! interval on the estimate; without it the estimate carries a degenerate
//! `(NANOS, NANOS)` interval and no standard deviation, so no dispersion is
//! fabricated.
//!
//! `--alloc-tracker OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]` writes an
//! `alloc_tracker` `<OPERATION>.json` file reporting `BYTES` mean bytes and `COUNT`
//! mean allocations per iteration; an optional `@BLOW:BHIGH/CLOW:CHIGH` suffix
//! additionally records a confidence interval (and matching slope and span count)
//! for the bytes and allocation-count metrics respectively. `--all-the-time
//! OPERATION=NANOS[@LOW:HIGH]` writes an `all_the_time` `<OPERATION>.json` file
//! reporting `NANOS` mean (and slope) processor-time nanoseconds per iteration; an
//! optional `@LOW:HIGH` suffix additionally records a confidence interval (and a
//! matching slope and span count).
//!
//! `--fail-if-exists PATH` writes no output and exits with code 1 when `PATH`
//! (resolved relative to the working directory) exists; otherwise it runs normally.
//! This lets a test designate specific commits as "the benchmark fails here": those
//! commits track the marker file, so when `cargo-bench-history` checks one out and
//! runs the faker the marker is present and the faker fails, standing in for a
//! commit that does not build or benchmark.
//!
//! `--chdir DIR` changes the working directory to `DIR` before resolving
//! `CARGO_TARGET_DIR` and writing output. It performs this directory change
//! internally to mirror cargo, which launches a benchmark binary with its working
//! directory set to the owning package's directory — so an engine that honors a
//! relative `CARGO_TARGET_DIR` (such as Criterion) resolves it from there.
//!
//! Output is written under `<target-root>`, which honors `CARGO_TARGET_DIR` exactly
//! as a harvest does.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// This binary is test-support scaffolding — a stand-in benchmark engine driven by
// in-workspace tests. Its own execution carries no product behavior worth a
// coverage signal, so coverage is disabled crate-wide rather than per function.
#![cfg_attr(coverage_nightly, coverage(off))]

use std::path::PathBuf;
use std::process::ExitCode;

use cargo_bench_history_faker::{
    AllocOperation, CallgrindCase, CriterionCase, TimeOperation, parse_alloc_arg,
    parse_callgrind_arg, parse_criterion_arg, parse_time_arg, write_all_the_time,
    write_alloc_tracker, write_callgrind_case, write_criterion_case,
};

// Install mimalloc as a scalable, general-purpose allocator process-wide: faster
// small allocations and no cross-thread allocator-lock contention (acute on the
// Windows process heap), a broad low-risk win applied uniformly across the
// workspace's binaries. Miri cannot call mimalloc's FFI, so under Miri the
// default allocator stands in.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let mut exit_code: u8 = 0;
    let mut callgrind_cases: Vec<CallgrindCase> = Vec::new();
    let mut criterion_cases: Vec<CriterionCase> = Vec::new();
    let mut alloc_operations: Vec<AllocOperation> = Vec::new();
    let mut time_operations: Vec<TimeOperation> = Vec::new();
    let mut fail_if_exists: Option<PathBuf> = None;
    let mut chdir: Option<PathBuf> = None;

    let mut args = faker_args(std::env::args()).into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-code" => {
                let value = args.next().expect("--exit-code requires a value");
                exit_code = value.parse().expect("--exit-code must be a u8");
            }
            "--callgrind" => {
                let value = args.next().expect(
                    "--callgrind requires GROUP|MODULE|FUNCTION[|ID[|PACKAGE_DIR]]=IR/BC/BI",
                );
                callgrind_cases.push(parse_callgrind_arg(&value));
            }
            "--criterion" => {
                let value = args
                    .next()
                    .expect("--criterion requires GROUP|FUNCTION|VALUE=NANOS[@STDDEV/CILOW:CIHIGH]");
                criterion_cases.push(parse_criterion_arg(&value));
            }
            "--alloc-tracker" => {
                let value = args
                    .next()
                    .expect("--alloc-tracker requires OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]");
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
            // A production `collect` run appends cargo scope flags (`--workspace`,
            // `--package`, `--bench`) and `--` passthrough after the faker's own
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

    for case in &callgrind_cases {
        write_callgrind_case(&target_root, case);
    }
    for case in &criterion_cases {
        write_criterion_case(&target_root, case);
    }
    for operation in &alloc_operations {
        write_alloc_tracker(&target_root, operation);
    }
    for operation in &time_operations {
        write_all_the_time(&target_root, operation);
    }

    ExitCode::from(exit_code)
}

/// Normalizes a raw process argument list into the faker's own arguments.
///
/// `argv[0]` is always the program name and is dropped. Cargo runs an external
/// subcommand as `cargo-<name> <name> <args...>`, so when invoked as
/// `cargo bench-history-faker` the first real argument is the subcommand name
/// `bench-history-faker`; it is dropped too, leaving the same list the faker sees
/// when run directly. Any other first argument is a real faker flag and is kept.
fn faker_args(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut rest: Vec<String> = raw.into_iter().skip(1).collect();
    if rest.first().map(String::as_str) == Some("bench-history-faker") {
        rest.remove(0);
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::faker_args;

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn drops_program_name_when_run_directly() {
        let args = faker_args(owned(&["cargo-bench-history-faker", "--exit-code", "1"]));
        assert_eq!(args, ["--exit-code", "1"]);
    }

    #[test]
    fn drops_cargo_subcommand_token_when_run_as_subcommand() {
        let args = faker_args(owned(&[
            "cargo-bench-history-faker",
            "bench-history-faker",
            "--exit-code",
            "1",
        ]));
        assert_eq!(args, ["--exit-code", "1"]);
    }
}
