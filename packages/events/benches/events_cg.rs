//! Callgrind benchmarks for the `events` crate's sync and local
//! manual-reset / auto-reset events.
//!
//! Paired with `events_uncontended.rs` (`signal_round_trip` group) which covers
//! the same `try_wait` operations under wall-clock measurement.
//!
//! Scenarios cover two orthogonal axes:
//!
//! * **manual-reset vs auto-reset** — different internal state machines.
//! * **sync vs local** — thread-safe atomics vs single-threaded `Cell`.
//!
//! Only the synchronous `try_wait` fast path is covered here. The async
//! `wait().poll()` fast path would require either heap-pinning the future
//! (`Box::pin`, which is dominated by allocation overhead) or storing a
//! self-referential future (impractical in the gungraun setup/return
//! shape). The wall-clock `async_poll_ready` Criterion group covers that
//! path with stack-pinned futures instead.
//!
//! Multi-threaded contention and blocking waits are intentionally out of
//! scope; both require a real scheduler that Callgrind cannot model.

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
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use events::{AutoResetEvent, LocalAutoResetEvent, LocalManualResetEvent, ManualResetEvent};
    use gungraun::prelude::*;

    fn make_set_manual() -> ManualResetEvent {
        let event = ManualResetEvent::boxed();
        event.set();
        event
    }

    fn make_set_local_manual() -> LocalManualResetEvent {
        let event = LocalManualResetEvent::boxed();
        event.set();
        event
    }

    fn make_fresh_auto() -> AutoResetEvent {
        AutoResetEvent::boxed()
    }

    fn make_fresh_local_auto() -> LocalAutoResetEvent {
        LocalAutoResetEvent::boxed()
    }

    // ---------- Sync (atomic) variants ----------

    // Pre-set event: each iteration consumes one try_wait. Manual events remain
    // set, so try_wait succeeds every time without any state mutation.
    #[library_benchmark]
    #[bench::set(make_set_manual())]
    fn manual_try_wait(event: ManualResetEvent) -> ManualResetEvent {
        _ = black_box(black_box(&event).try_wait());
        event
    }

    // Auto-reset is a set+take cycle: each iteration sets the event and
    // immediately consumes the signal via try_wait, leaving the event in its
    // original reset state. This matches `events_bench::signal_round_trip
    // events/AutoResetEvent`.
    #[library_benchmark]
    #[bench::fresh(make_fresh_auto())]
    fn auto_set_then_try_wait(event: AutoResetEvent) -> AutoResetEvent {
        black_box(&event).set();
        _ = black_box(black_box(&event).try_wait());
        event
    }

    // ---------- Local (non-atomic) variants ----------

    #[library_benchmark]
    #[bench::set(make_set_local_manual())]
    fn local_manual_try_wait(event: LocalManualResetEvent) -> LocalManualResetEvent {
        _ = black_box(black_box(&event).try_wait());
        event
    }

    #[library_benchmark]
    #[bench::fresh(make_fresh_local_auto())]
    fn local_auto_set_then_try_wait(event: LocalAutoResetEvent) -> LocalAutoResetEvent {
        black_box(&event).set();
        _ = black_box(black_box(&event).try_wait());
        event
    }

    library_benchmark_group!(
        name = sync_group,
        benchmarks = [manual_try_wait, auto_set_then_try_wait]
    );

    library_benchmark_group!(
        name = local_group,
        benchmarks = [local_manual_try_wait, local_auto_set_then_try_wait]
    );
}

#[cfg(target_os = "linux")]
pub use linux::{local_group, sync_group};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = sync_group, local_group);
