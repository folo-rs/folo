//! Basic operations on the `thread_local_arc!` macro and underlying type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, LazyLock};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[expect(
    dead_code,
    reason = "We do not care about using all the fields but we want to pay the price of initializing them"
)]
#[linked::object]
struct TestSubject {
    local_state: AtomicUsize,
    shared_state: Arc<AtomicUsize>,
}

impl TestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));

        linked::new!(Self {
            local_state: AtomicUsize::new(0),
            shared_state: Arc::clone(&shared_state),
        })
    }
}

linked::thread_local_arc!(static TARGET: TestSubject = TestSubject::new());

static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(&ProcessorSet::single());
    let mut two_threads = TWO_PROCESSORS.as_ref().map(ThreadPool::new);

    let mut g = c.benchmark_group("thread_local_arc::access_one_threaded");

    Run::new()
        .iter(|_| TARGET.with(|val| Arc::weak_count(&val.shared_state)))
        .execute_criterion_on(&mut one_thread, &mut g, "with");

    Run::new()
        .iter(|_| Arc::weak_count(&TARGET.to_arc().shared_state))
        .execute_criterion_on(&mut one_thread, &mut g, "to_arc");

    // For comparison, we also include a thread_local! LazyCell.
    Run::new()
        .iter(|_| {
            TEST_SUBJECT_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state))
        })
        .execute_criterion_on(&mut one_thread, &mut g, "vs_std_thread_local");

    // For comparison, we also include a global LazyLock.
    Run::new()
        .iter(|_| Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state))
        .execute_criterion_on(&mut one_thread, &mut g, "vs_static_lazy_lock");

    g.finish();

    if let Some(ref mut two_threads) = two_threads {
        let mut g = c.benchmark_group("thread_local_arc::access_two_threaded");

        Run::new()
            .iter(|_| TARGET.with(|val| Arc::weak_count(&val.shared_state)))
            .execute_criterion_on(two_threads, &mut g, "with");

        Run::new()
            .iter(|_| Arc::weak_count(&TARGET.to_arc().shared_state))
            .execute_criterion_on(two_threads, &mut g, "to_arc");

        // For comparison, we also include a thread_local! LazyCell.
        Run::new()
            .iter(|_| {
                TEST_SUBJECT_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state))
            })
            .execute_criterion_on(two_threads, &mut g, "vs_std_thread_local");

        // For comparison, we also include a global LazyLock.
        Run::new()
            .iter(|_| Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state))
            .execute_criterion_on(two_threads, &mut g, "vs_static_lazy_lock");

        g.finish();
    }
}

// Rough equivalent of the above TestSubject, to compare non-linked object via LazyLock.
struct ComparisonTestSubject {
    _local_state: AtomicUsize,
    shared_state: Arc<AtomicUsize>,
}

impl ComparisonTestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));

        Self {
            _local_state: AtomicUsize::new(0),
            shared_state,
        }
    }
}

thread_local! {
    static TEST_SUBJECT_THREAD_LOCAL: TestSubject = TestSubject::new();
}

static TEST_SUBJECT_GLOBAL: LazyLock<ComparisonTestSubject> =
    LazyLock::new(ComparisonTestSubject::new);
