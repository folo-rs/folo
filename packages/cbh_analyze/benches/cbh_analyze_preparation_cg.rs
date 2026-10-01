//! Deterministic candidate-selection counterpart to `cbh_analyze_preparation.rs`.
//!
//! Folding retains randomized production hashers; its instruction counts would
//! vary without code changes, so it is intentionally measured only by Criterion.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "These lints originate in Gungraun macro expansion"
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, main};

#[cfg(target_os = "linux")]
main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes", "--collect-bus=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    ),
    library_benchmark_groups = candidates
);

::testing::set_allocator!();

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use cbh_analyze::benchmarks::{
        CandidateFixture, CandidateOutput, HIGH_CANDIDATE_BATCHES, LOW_CANDIDATE_BATCHES,
    };
    use gungraun::{library_benchmark, library_benchmark_group};

    fn setup(batches: usize) -> (CandidateFixture, Vec<String>) {
        let fixture = CandidateFixture::new(batches);
        let keys = fixture.keys();
        (fixture, keys)
    }

    #[library_benchmark]
    #[bench::keys_64(setup(LOW_CANDIDATE_BATCHES))]
    #[bench::keys_512(setup(HIGH_CANDIDATE_BATCHES))]
    fn candidates_filter(
        input: (CandidateFixture, Vec<String>),
    ) -> (CandidateFixture, CandidateOutput) {
        let (fixture, keys) = input;
        let output = fixture.select(black_box(keys));
        // Retain fixture ownership in the returned value instead of charging its
        // teardown to filtering; the equivalent Criterion fixture lives outside iter.
        black_box((fixture, output))
    }

    library_benchmark_group!(name = candidates, benchmarks = [candidates_filter]);
}
