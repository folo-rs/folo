//! Basic operations on the `InstancePerThreadSync` wrapper type, used directly for dynamic storage.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, LazyLock};

use criterion::{Criterion, criterion_group, criterion_main};
use linked::InstancePerThreadSync;
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

static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

fn entrypoint(c: &mut Criterion) {
    local(c);
    local_ref(c);
    local_ref_multithreaded(c);
    local_ref_access(c);
}

fn local(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(ProcessorSet::single());

    let mut g = c.benchmark_group("instance_per_thread_sync::create");

    Run::new()
        .prepare_iter(|_| TestSubject::new())
        .iter(|mut args| InstancePerThreadSync::new(args.take_iter_state()))
        .execute_criterion_on(&mut one_thread, &mut g, "new");

    let per_thread = InstancePerThreadSync::new(TestSubject::new());

    Run::new()
        .iter(|_| per_thread.clone())
        .execute_criterion_on(&mut one_thread, &mut g, "clone");

    g.finish();
}

fn local_ref(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(ProcessorSet::single());

    let mut g = c.benchmark_group("instance_per_thread_sync::Ref");

    let per_thread = InstancePerThreadSync::new(TestSubject::new());

    Run::new()
        .iter(|_| per_thread.acquire())
        .execute_criterion_on(&mut one_thread, &mut g, "new_single");

    {
        // We keep one InstancePerThreadSync here so the ones we create in iterations are not the only ones.
        let _first = per_thread.acquire();

        Run::new()
            .iter(|_| per_thread.acquire())
            .execute_criterion_on(&mut one_thread, &mut g, "new_not_single");
    }

    {
        Run::new()
            .prepare_thread(|_| per_thread.acquire())
            .iter(|args| args.thread_state().clone())
            .execute_criterion_on(&mut one_thread, &mut g, "clone");
    }

    g.finish();
}

fn local_ref_multithreaded(c: &mut Criterion) {
    let mut two_threads = TWO_PROCESSORS.as_ref().map(ThreadPool::new);

    if let Some(ref mut two_threads) = two_threads {
        let mut g = c.benchmark_group("instance_per_thread_sync::Ref::two-threaded");

        let per_thread = InstancePerThreadSync::new(TestSubject::new());

        Run::new()
            .iter(|_| per_thread.acquire())
            .execute_criterion_on(two_threads, &mut g, "new_single");

        Run::new()
            .prepare_thread(|_| {
                // We just keep the first instance around during the benchmark, so any additional
                // `acquire` calls operate in a non-clean state with an already existing instance.
                per_thread.acquire()
            })
            .iter(|_| per_thread.acquire())
            .execute_criterion_on(two_threads, &mut g, "new_not_single");

        Run::new()
            .prepare_thread(|_| per_thread.acquire())
            .iter(|args| args.thread_state().clone())
            .execute_criterion_on(two_threads, &mut g, "clone");

        g.finish();
    }
}

fn local_ref_access(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(ProcessorSet::single());
    let mut two_threads = TWO_PROCESSORS.as_ref().map(ThreadPool::new);

    let mut g = c.benchmark_group("instance_per_thread_sync::Ref::access");

    let per_thread = InstancePerThreadSync::new(TestSubject::new());

    Run::new()
        .prepare_thread(|_| per_thread.acquire())
        .iter(|args| Arc::weak_count(&args.thread_state().shared_state))
        .execute_criterion_on(&mut one_thread, &mut g, "deref");

    // For comparison, we also include a thread_local! LazyCell.
    Run::new()
        .iter(|_| TEST_SUBJECT_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state)))
        .execute_criterion_on(&mut one_thread, &mut g, "vs_std_thread_local");

    // For comparison, we also include a global LazyLock.
    Run::new()
        .iter(|_| Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state))
        .execute_criterion_on(&mut one_thread, &mut g, "vs_static_lazy_lock");

    if let Some(ref mut two_threads) = two_threads {
        // For comparison, also the LazyLock in multithreaded mode, as all the other
        // ones are thread-local and have no MT overhead but this may have overhead.
        Run::new()
            .iter(|_| Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state))
            .execute_criterion_on(two_threads, &mut g, "vs_static_lazy_lock_mt");
    }

    g.finish();
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
