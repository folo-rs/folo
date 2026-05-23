//! Cycle-accurate benchmarks for the `events` crate's sync and local
//! manual-reset / auto-reset events.
//!
//! Paired with `events_bench.rs` (`signal_round_trip` group) which covers
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
    allow(
        clippy::absolute_paths,
        clippy::allow_attributes_without_reason,
        clippy::exhaustive_structs,
        clippy::partial_pub_fields,
        clippy::pub_underscore_fields,
        clippy::cognitive_complexity,
        clippy::unnecessary_wraps,
        clippy::ignore_without_reason,
        clippy::default_trait_access,
        clippy::needless_pass_by_value,
        clippy::missing_assert_message,
        clippy::elidable_lifetime_names,
        clippy::needless_pass_by_ref_mut,
        clippy::doc_markdown,
        clippy::needless_for_each,
        clippy::redundant_clone,
        clippy::missing_docs_in_private_items,
        clippy::exit,
        clippy::undocumented_unsafe_blocks,
        unused_imports,
        unused_qualifications,
        unreachable_pub,
        missing_debug_implementations,
        unnameable_types,
        non_local_definitions,
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
extern crate gungraun;

#[cfg(target_os = "linux")]
use std::hint::black_box;

#[cfg(target_os = "linux")]
use events::{AutoResetEvent, LocalAutoResetEvent, LocalManualResetEvent, ManualResetEvent};
#[cfg(target_os = "linux")]
use gungraun::prelude::*;

#[cfg(target_os = "linux")]
fn make_set_manual() -> ManualResetEvent {
    let event = ManualResetEvent::boxed();
    event.set();
    event
}

#[cfg(target_os = "linux")]
fn make_set_local_manual() -> LocalManualResetEvent {
    let event = LocalManualResetEvent::boxed();
    event.set();
    event
}

#[cfg(target_os = "linux")]
fn make_fresh_auto() -> AutoResetEvent {
    AutoResetEvent::boxed()
}

#[cfg(target_os = "linux")]
fn make_fresh_local_auto() -> LocalAutoResetEvent {
    LocalAutoResetEvent::boxed()
}

// ---------- Sync (atomic) variants ----------

// Pre-set event: each iteration consumes one try_wait. Manual events remain
// set, so try_wait succeeds every time without any state mutation.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::fresh(make_fresh_auto())]
fn auto_set_then_try_wait(event: AutoResetEvent) -> AutoResetEvent {
    black_box(&event).set();
    _ = black_box(black_box(&event).try_wait());
    event
}

// ---------- Local (non-atomic) variants ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::set(make_set_local_manual())]
fn local_manual_try_wait(event: LocalManualResetEvent) -> LocalManualResetEvent {
    _ = black_box(black_box(&event).try_wait());
    event
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::fresh(make_fresh_local_auto())]
fn local_auto_set_then_try_wait(event: LocalAutoResetEvent) -> LocalAutoResetEvent {
    black_box(&event).set();
    _ = black_box(black_box(&event).try_wait());
    event
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = sync_group,
    benchmarks = [manual_try_wait, auto_set_then_try_wait]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = local_group,
    benchmarks = [local_manual_try_wait, local_auto_set_then_try_wait]
);

#[cfg(target_os = "linux")]
main!(library_benchmark_groups = sync_group, local_group);
