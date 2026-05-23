//! Cycle-accurate benchmarks for the `awaiter_set` crate.
//!
//! Paired with `awaiter_set_bench.rs` (round-trip Criterion scenarios)
//! which covers the same operations under wall-clock measurement.
//!
//! Scenarios isolate the individual operations that make up the awaiter
//! lifecycle so each can be tracked at instruction-level granularity:
//!
//! * `register_into_empty` — first registration into a fresh set.
//! * `register_appending_to_10` — registration onto a populated tail.
//! * `notify_one_singleton` — pop the only awaiter, set becomes empty.
//! * `unregister_only` — unlink the only awaiter, set becomes empty.
//! * `take_notification_when_notified` — `take_notification` CAS success.
//! * `take_notification_when_waiting` — `take_notification` CAS failure.
//!
//! Multi-threaded contention is intentionally out of scope; callers
//! synchronize external to the set, and Callgrind cannot model cache
//! coherence traffic meaningfully.

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
        clippy::multiple_unsafe_ops_per_block,
        unused_imports,
        unused_qualifications,
        dead_code,
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
use std::pin::Pin;
#[cfg(target_os = "linux")]
use std::task::Waker;

#[cfg(target_os = "linux")]
use awaiter_set::{Awaiter, AwaiterSet};
#[cfg(target_os = "linux")]
use gungraun::prelude::*;

// Bundles an awaiter and the set that holds (or will hold) it. The
// field order is important — `awaiter` drops before `set`, but since
// `AwaiterSet` has no Drop impl, no UAF occurs even if the set still
// holds raw pointers to the freed awaiter.
#[cfg(target_os = "linux")]
struct SoloState {
    awaiter: Pin<Box<Awaiter>>,
    set: AwaiterSet,
}

// Bundles a fresh awaiter to register, plus a set already populated
// with `anchors` (kept alive so registered raw pointers stay valid for
// the duration of the bench). Drop order: `target` -> `anchors` -> `set`,
// which is harmless because `AwaiterSet` has no Drop impl.
#[cfg(target_os = "linux")]
struct PopulatedState {
    target: Pin<Box<Awaiter>>,
    anchors: Vec<Pin<Box<Awaiter>>>,
    set: AwaiterSet,
}

#[cfg(target_os = "linux")]
fn make_empty_solo() -> SoloState {
    SoloState {
        awaiter: Box::pin(Awaiter::new()),
        set: AwaiterSet::new(),
    }
}

#[cfg(target_os = "linux")]
fn make_populated_10() -> PopulatedState {
    const ANCHOR_COUNT: usize = 10;
    let mut anchors: Vec<Pin<Box<Awaiter>>> = std::iter::repeat_with(|| Box::pin(Awaiter::new()))
        .take(ANCHOR_COUNT)
        .collect();
    let mut set = AwaiterSet::new();
    for anchor in &mut anchors {
        // SAFETY: `anchor` is heap-pinned and kept alive in the returned
        // state for the entire bench, satisfying `register`'s contract.
        unsafe {
            set.register(anchor.as_mut(), Waker::noop().clone());
        }
    }
    PopulatedState {
        target: Box::pin(Awaiter::new()),
        anchors,
        set,
    }
}

// Set already contains the awaiter -- bench measures `notify_one` against
// the singleton.
#[cfg(target_os = "linux")]
fn make_registered_solo() -> SoloState {
    let mut state = make_empty_solo();
    // SAFETY: The awaiter is heap-pinned and remains alive in the
    // returned state for the entire bench.
    unsafe {
        state
            .set
            .register(state.awaiter.as_mut(), Waker::noop().clone());
    }
    state
}

// Set already contains the awaiter, then the awaiter was notified --
// `take_notification` should CAS-success transition NOTIFIED -> IDLE.
#[cfg(target_os = "linux")]
fn make_notified_solo() -> SoloState {
    let mut state = make_registered_solo();
    drop(state.set.notify_one());
    state
}

// ---------- Registration ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::empty(make_empty_solo())]
fn register_into_empty(mut state: SoloState) -> SoloState {
    // SAFETY: The awaiter is heap-pinned, kept alive in the returned
    // state until after the measured region, and not currently
    // registered.
    unsafe {
        state
            .set
            .register(state.awaiter.as_mut(), Waker::noop().clone());
    }
    state
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::with_10(make_populated_10())]
fn register_appending_to_10(mut state: PopulatedState) -> PopulatedState {
    // SAFETY: `state.target` is heap-pinned, kept alive in the returned
    // state, and not currently registered.
    unsafe {
        state
            .set
            .register(state.target.as_mut(), Waker::noop().clone());
    }
    state
}

// ---------- Removal via notification ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::singleton(make_registered_solo())]
fn notify_one_singleton(mut state: SoloState) -> SoloState {
    drop(black_box(state.set.notify_one()));
    state
}

// ---------- Removal via unregister ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::singleton(make_registered_solo())]
fn unregister_only(mut state: SoloState) -> SoloState {
    // SAFETY: The awaiter is registered with this set (per setup) and
    // remains pinned and alive in the returned state.
    unsafe {
        state.set.unregister(state.awaiter.as_mut());
    }
    state
}

// ---------- Atomic-only paths ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::notified(make_notified_solo())]
fn take_notification_when_notified(state: SoloState) -> SoloState {
    _ = black_box(state.awaiter.as_ref().take_notification());
    state
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::waiting(make_registered_solo())]
fn take_notification_when_waiting(state: SoloState) -> SoloState {
    _ = black_box(state.awaiter.as_ref().take_notification());
    state
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = register_group,
    benchmarks = [register_into_empty, register_appending_to_10]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = removal_group,
    benchmarks = [notify_one_singleton, unregister_only]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = atomic_group,
    benchmarks = [
        take_notification_when_notified,
        take_notification_when_waiting,
    ]
);

#[cfg(target_os = "linux")]
main!(
    library_benchmark_groups = register_group,
    removal_group,
    atomic_group
);
