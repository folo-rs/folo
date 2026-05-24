//! Callgrind benchmarks for `fast_time::Clock` operations.
//!
//! Paired with `fast_time_timestamp_performance.rs` which covers the same operations
//! under wall-clock measurement.
//!
//! # Scope and caveats
//!
//! `Clock::now()` ultimately reaches a kernel time source (Windows
//! `GetSystemTimePreciseAsFileTime` / `QueryUnbiasedInterruptTime`, Linux
//! `clock_gettime(CLOCK_MONOTONIC_COARSE)`). Callgrind models syscall cost
//! as essentially-free, so the **instruction counts measured here capture
//! only the user-space wrapper around the syscall**, not the real cost of
//! retrieving the time. This is still useful: an unintended regression in
//! the wrapper (extra branches, redundant work, layout changes) will show
//! up here even when wall-clock measurements bury it in syscall noise.
//!
//! The wall-clock benchmarks in `fast_time_timestamp_performance.rs` remain the
//! source of truth for the actual end-to-end cost of `Clock::now()`. See
//! `docs/callgrind-benchmarks.md` for the broader pairing rule.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only. On other platforms this bench target compiles
    // to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::time::Instant as StdInstant;

    use fast_time::{Clock, Instant};
    use gungraun::prelude::*;

    fn make_clock() -> Clock {
        Clock::new()
    }

    fn make_clock_with_anchor() -> (Clock, Instant) {
        let mut clock = Clock::new();
        let anchor = clock.now();
        (clock, anchor)
    }

    fn make_two_instants() -> (Instant, Instant) {
        let mut clock = Clock::new();
        let first = clock.now();
        let second = clock.now();
        (first, second)
    }

    // Headline measurement: the user-space wrapper around the kernel time
    // source. Compare against `std_now` below to see how much wrapper the
    // standard library adds.
    #[library_benchmark]
    #[bench::fresh(make_clock())]
    fn clock_now(mut clock: Clock) -> Clock {
        _ = black_box(black_box(&mut clock).now());
        clock
    }

    // `Instant::elapsed` calls `now()` internally and then does arithmetic.
    #[library_benchmark]
    #[bench::after_one_tick(make_clock_with_anchor())]
    fn instant_elapsed(input: (Clock, Instant)) -> (Clock, Instant) {
        let (mut clock, anchor) = input;
        _ = black_box(black_box(&anchor).elapsed(black_box(&mut clock)));
        (clock, anchor)
    }

    // Pure arithmetic, no syscall. Useful as a baseline for the arithmetic
    // overhead that `elapsed` adds on top of `now`.
    #[library_benchmark]
    #[bench::two_instants(make_two_instants())]
    fn instant_saturating_duration_since(input: (Instant, Instant)) -> (Instant, Instant) {
        let (first, second) = input;
        _ = black_box(black_box(&second).saturating_duration_since(black_box(first)));
        (first, second)
    }

    // Sibling comparison: how many wrapper instructions does `std::time::Instant::now()`
    // add on top of its own syscall? The delta against `clock_now` is the value
    // proposition of `fast_time` (no allocator, no monotonic-correction logic).
    #[library_benchmark]
    fn std_now() {
        _ = black_box(StdInstant::now());
    }

    library_benchmark_group!(
        name = clock_group,
        benchmarks = [
            clock_now,
            instant_elapsed,
            instant_saturating_duration_since,
            std_now,
        ]
    );
}

#[cfg(target_os = "linux")]
pub use linux::clock_group;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = clock_group);
