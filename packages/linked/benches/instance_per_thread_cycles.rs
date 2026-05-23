//! Cycle-accurate benchmarks for the `linked` crate's `InstancePerThread`
//! handle hot path.
//!
//! Paired with `instance_per_thread.rs` (the `instance_per_thread::Ref`
//! and `instance_per_thread::Ref::access` Criterion groups) which cover
//! the same operations under wall-clock measurement.
//!
//! Scenarios isolate the per-call cost of acquiring and using thread-
//! local handles so each can be tracked at instruction-level granularity:
//!
//! * `acquire_first_touch` — first acquire on the thread; allocates the
//!   thread-local instance.
//! * `acquire_cached` — subsequent acquire; hits the thread-local cache.
//! * `ref_clone` — clone an existing `Ref<T>`.
//! * `ref_field_access` — deref through a `Ref<T>` and access a field.
//! * `std_thread_local_access_first_touch` — `thread_local!` `LazyCell`
//!   first-touch baseline.
//! * `std_thread_local_access_primed` — `thread_local!` `LazyCell`
//!   cached-access baseline.

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
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicUsize;

#[cfg(target_os = "linux")]
use gungraun::prelude::*;
#[cfg(target_os = "linux")]
use linked::{InstancePerThread, Ref};

#[cfg(target_os = "linux")]
#[expect(
    dead_code,
    reason = "fields are paid for at construction, accessed in one scenario only"
)]
#[linked::object]
struct TestSubject {
    local_state: AtomicUsize,
    shared_state: Arc<AtomicUsize>,
}

#[cfg(target_os = "linux")]
impl TestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));
        linked::new!(Self {
            local_state: AtomicUsize::new(0),
            shared_state: Arc::clone(&shared_state),
        })
    }
}

#[cfg(target_os = "linux")]
struct PrimedState {
    per_thread: InstancePerThread<TestSubject>,
    // Keeping this anchor ref alive ensures `acquire` calls go through
    // the cached path rather than the first-touch (initialization) path.
    _anchor: Ref<TestSubject>,
}

#[cfg(target_os = "linux")]
struct RefState {
    _per_thread: InstancePerThread<TestSubject>,
    handle: Ref<TestSubject>,
}

#[cfg(target_os = "linux")]
fn make_fresh() -> InstancePerThread<TestSubject> {
    InstancePerThread::new(TestSubject::new())
}

#[cfg(target_os = "linux")]
fn make_primed() -> PrimedState {
    let per_thread = InstancePerThread::new(TestSubject::new());
    let anchor = per_thread.acquire();
    PrimedState {
        per_thread,
        _anchor: anchor,
    }
}

#[cfg(target_os = "linux")]
fn make_ref() -> RefState {
    let per_thread = InstancePerThread::new(TestSubject::new());
    let handle = per_thread.acquire();
    RefState {
        _per_thread: per_thread,
        handle,
    }
}

// std thread_local! baseline. `with()` returns a value from the closure;
// we read a field's `Arc::weak_count` to match the Criterion comparison.
#[cfg(target_os = "linux")]
struct ComparisonSubject {
    _local_state: AtomicUsize,
    shared_state: Arc<AtomicUsize>,
}

#[cfg(target_os = "linux")]
impl ComparisonSubject {
    fn new() -> Self {
        Self {
            _local_state: AtomicUsize::new(0),
            shared_state: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[cfg(target_os = "linux")]
thread_local! {
    static STD_THREAD_LOCAL: ComparisonSubject = ComparisonSubject::new();
}

// ---------- Acquire paths ----------

// Setup creates a fresh InstancePerThread but does NOT acquire — so the
// bench's acquire is the first one on this thread and pays the
// initialization cost.
#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::fresh(make_fresh())]
fn acquire_first_touch(
    per_thread: InstancePerThread<TestSubject>,
) -> (InstancePerThread<TestSubject>, Ref<TestSubject>) {
    let handle = black_box(&per_thread).acquire();
    (per_thread, handle)
}

// Setup primes the thread-local cache so the bench's acquire hits the
// cached fast path.
#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::primed(make_primed())]
fn acquire_cached(state: PrimedState) -> (PrimedState, Ref<TestSubject>) {
    let handle = black_box(&state.per_thread).acquire();
    (state, handle)
}

// ---------- Ref operations ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::ready(make_ref())]
fn ref_clone(state: RefState) -> (RefState, Ref<TestSubject>) {
    let cloned = black_box(&state.handle).clone();
    (state, cloned)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::ready(make_ref())]
fn ref_field_access(state: RefState) -> RefState {
    let count = Arc::weak_count(&black_box(&state.handle).shared_state);
    _ = black_box(count);
    state
}

// ---------- Baseline ----------

// Setup primes the std thread_local LazyCell so the bench measures the
// cached access path, matching the primed `acquire_cached` benchmark.
#[cfg(target_os = "linux")]
fn prime_std_thread_local() {
    _ = STD_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state));
}

#[cfg(target_os = "linux")]
#[library_benchmark]
fn std_thread_local_access_first_touch() -> usize {
    STD_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state))
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::primed(prime_std_thread_local())]
fn std_thread_local_access_primed(_: ()) -> usize {
    STD_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state))
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = acquire_group,
    benchmarks = [acquire_first_touch, acquire_cached]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(name = ref_group, benchmarks = [ref_clone, ref_field_access]);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = baseline_group,
    benchmarks = [
        std_thread_local_access_first_touch,
        std_thread_local_access_primed,
    ]
);

#[cfg(target_os = "linux")]
main!(
    library_benchmark_groups = acquire_group,
    ref_group,
    baseline_group
);
