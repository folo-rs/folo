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
use par_bench::{ConfiguredRun, ThreadPool};

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

static ONE_PROCESSOR: LazyLock<ProcessorSet> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(1)).unwrap());
static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
    LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

static ONE_THREAD: LazyLock<ThreadPool> = LazyLock::new(|| ThreadPool::new(&ONE_PROCESSOR));
static TWO_THREADS: LazyLock<Option<ThreadPool>> =
    LazyLock::new(|| TWO_PROCESSORS.as_ref().map(ThreadPool::new));

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("thread_local_arc::access_one_threaded");

    g.bench_function("with", |b| {
        b.iter_custom(|iters| {
            ConfiguredRun::builder()
                .iter_fn(|()| TARGET.with(|val| Arc::weak_count(&val.shared_state)))
                .build()
                .execute_on(&ONE_THREAD, iters)
                .mean_duration()
        });
    });

    g.bench_function("to_arc", |b| {
        b.iter_custom(|iters| {
            ConfiguredRun::builder()
                .iter_fn(|()| Arc::weak_count(&TARGET.to_arc().shared_state))
                .build()
                .execute_on(&ONE_THREAD, iters)
                .mean_duration()
        });
    });

    // For comparison, we also include a thread_local! LazyCell.
    g.bench_function("vs_std_thread_local", |b| {
        b.iter_custom(|iters| {
            ConfiguredRun::builder()
                .iter_fn(|()| {
                    TEST_SUBJECT_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state))
                })
                .build()
                .execute_on(&ONE_THREAD, iters)
                .mean_duration()
        });
    });

    // For comparison, we also include a global LazyLock.
    g.bench_function("vs_static_lazy_lock", |b| {
        b.iter_custom(|iters| {
            ConfiguredRun::builder()
                .iter_fn(|()| Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state))
                .build()
                .execute_on(&ONE_THREAD, iters)
                .mean_duration()
        });
    });

    g.finish();

    if let Some(two_threads) = &*TWO_THREADS {
        let mut g = c.benchmark_group("thread_local_arc::access_two_threaded");

        g.bench_function("with", |b| {
            b.iter_custom(|iters| {
                ConfiguredRun::builder()
                    .iter_fn(|()| TARGET.with(|val| Arc::weak_count(&val.shared_state)))
                    .build()
                    .execute_on(two_threads, iters)
                    .mean_duration()
            });
        });

        g.bench_function("to_arc", |b| {
            b.iter_custom(|iters| {
                ConfiguredRun::builder()
                    .iter_fn(|()| Arc::weak_count(&TARGET.to_arc().shared_state))
                    .build()
                    .execute_on(two_threads, iters)
                    .mean_duration()
            });
        });

        // For comparison, we also include a thread_local_arc! LazyCell.
        g.bench_function("vs_std_thread_local", |b| {
            b.iter_custom(|iters| {
                ConfiguredRun::builder()
                    .iter_fn(|()| {
                        TEST_SUBJECT_THREAD_LOCAL.with(|local| Arc::weak_count(&local.shared_state))
                    })
                    .build()
                    .execute_on(two_threads, iters)
                    .mean_duration()
            });
        });

        // For comparison, we also include a global LazyLock.
        g.bench_function("vs_static_lazy_lock", |b| {
            b.iter_custom(|iters| {
                ConfiguredRun::builder()
                    .iter_fn(|()| Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state))
                    .build()
                    .execute_on(two_threads, iters)
                    .mean_duration()
            });
        });

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
