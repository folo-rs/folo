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
//! cargo-bench-history-faker [--exit-code N] [--summary GROUP=KIND]...
//!                           [--criterion GROUP|FUNCTION|VALUE=NANOS]...
//!                           [--alloc-tracker OPERATION=BYTES/COUNT[@BLOW:BHIGH/CLOW:CHIGH]]...
//!                           [--all-the-time OPERATION=NANOS[@LOW:HIGH]]...
//!                           [--fail-if-exists PATH] [--chdir DIR]
//! ```
//!
//! `--summary GROUP=KIND` writes a Gungraun (Callgrind) `summary.json`, where
//! `KIND` is one of `single`, `parametrized`, or `single-alt-pkg` (see
//! [`callgrind_preset`](cargo_bench_history_faker::callgrind_preset)). `GROUP` may
//! contain `/`, which is split into nested directory segments under `gungraun/`.
//! Real Gungraun output nests by bench binary (`gungraun/<binary>/<group>/...`), so
//! two same-named bench harnesses in different packages land under the same
//! top-level directory but in distinct nested ones — the on-disk collision shape a
//! recursive harvest must keep distinct.
//!
//! `--criterion GROUP|FUNCTION|VALUE=NANOS` writes a Criterion case as a
//! `new/benchmark.json` and `new/estimates.json` pair whose identity is
//! `GROUP`/`FUNCTION`/`VALUE` (an empty `VALUE` omits the parameter component) and
//! whose wall-clock slope estimate is `NANOS` nanoseconds.
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
//! `--fail-if-exists PATH` exits with code 1 and writes no output when `PATH`
//! (relative to the working directory) exists. Backfill runs each engine in a
//! checked-out worktree, so a commit that tracks the named marker file stands in
//! for a commit that fails to build or benchmark.
//!
//! `--chdir DIR` changes the working directory to `DIR` before resolving
//! `CARGO_TARGET_DIR` and writing output. This stands in for cargo, which runs a
//! benchmark binary with its working directory set to the owning package's
//! directory — so an engine that honors a relative `CARGO_TARGET_DIR` (such as
//! Criterion) would resolve it there.
//!
//! Output is written under `<target-root>`, which honors `CARGO_TARGET_DIR` exactly
//! as a harvest does.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::path::PathBuf;
use std::process::ExitCode;

use cargo_bench_history_faker::{
    AllocOperation, CriterionCase, TimeOperation, callgrind_preset, parse_alloc_arg,
    parse_criterion_arg, parse_time_arg, write_all_the_time, write_alloc_tracker,
    write_callgrind_summary, write_criterion_case,
};

// Install mimalloc as a scalable, general-purpose allocator process-wide: faster
// small allocations and no cross-thread allocator-lock contention (acute on the
// Windows process heap), a broad low-risk win applied uniformly across the
// workspace's binaries. Miri cannot call mimalloc's FFI, so under Miri the
// default allocator stands in.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
                summaries.push((group.to_owned(), callgrind_preset(kind)));
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

    for (group, content) in &summaries {
        write_callgrind_summary(&target_root, group, content);
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
