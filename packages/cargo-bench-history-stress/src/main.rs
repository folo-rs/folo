#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the `cargo-bench-history` stress harness.
//!
//! All logic lives in the library crate; this shim only forwards to
//! [`cargo_bench_history_stress::run`] so the orchestration is exercised through
//! the library (covered and mutation-tested) rather than the binary target.

use std::process::ExitCode;

fn main() -> ExitCode {
    cargo_bench_history_stress::run()
}
