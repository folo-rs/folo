#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the `cargo-bench-history` stress harness.
//!
//! All logic lives in the library crate; this shim only forwards to
//! [`cargo_bench_history_stress::run`] so the orchestration is exercised through
//! the library (covered and mutation-tested) rather than the binary target.

use std::process::ExitCode;

// The harness must measure the same allocator the production binary ships, so its
// `analyze`-path numbers stay representative: the chunked parallel parse only scales
// with a per-thread-heap allocator (see `cargo-bench-history`'s `main`). Miri cannot
// call mimalloc's FFI, so under Miri the default allocator stands in.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    cargo_bench_history_stress::run()
}
