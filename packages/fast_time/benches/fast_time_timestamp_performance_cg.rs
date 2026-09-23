//! Callgrind benchmarks for timestamp capture in `fast_time`.
//!
//! Paired with `fast_time_timestamp_performance.rs`: `timestamp_capture_clock_now`
//! corresponds to `timestamp_capture/fast_time_clock/now`, and `timestamp_capture_std_now`
//! corresponds to `timestamp_capture/std_instant/now`.
//!
//! # Scope and caveats
//!
//! These counts cover user-space time-source wrappers, not the actual cost of
//! retrieving time from the operating system. Criterion remains the source of
//! truth for timestamp-capture latency; a wrapper instruction delta is not a
//! comparison of time-source performance.
//!
//! The fast-time case isolates the first capture from a fresh clock, whereas its
//! Criterion counterpart measures repeated captures from a warmed clock. Repeated
//! real-clock captures and `Instant::elapsed` depend on coarse-clock ticks and
//! the resulting cache branches, so they do not provide a stable instruction
//! baseline. `Instant` duration arithmetic forwards to the standard library
//! rather than providing a separate fast-time algorithm to track.

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
        reason = "These lints originate in Gungraun macro expansion and cannot be addressed in \
          this benchmark."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only. On other platforms this bench target compiles
    // to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, main};
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes", "--collect-bus=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    ),
    library_benchmark_groups = timestamp_capture
);

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::time::Instant as StdInstant;

    use fast_time::Clock;
    use gungraun::prelude::*;

    fn make_clock() -> Clock {
        Clock::new()
    }

    #[library_benchmark]
    #[bench::fresh(make_clock())]
    fn timestamp_capture_clock_now(mut clock: Clock) -> Clock {
        _ = black_box(black_box(&mut clock).now());
        clock
    }

    #[library_benchmark]
    fn timestamp_capture_std_now() {
        _ = black_box(StdInstant::now());
    }

    library_benchmark_group!(
        name = timestamp_capture,
        benchmarks = [timestamp_capture_clock_now, timestamp_capture_std_now,]
    );
}
