//! Basic operations on the `thread_local_arc!` macro and underlying type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::{
    hint::black_box,
    sync::{Arc, LazyLock, atomic::AtomicUsize},
};

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{Criterion, criterion_group, criterion_main};

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

fn entrypoint(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    let mut g = c.benchmark_group("thread_local_arc::access_single_threaded");

    g.bench_function("with", |b| {
        b.iter(|| black_box(TARGET.with(|val| Arc::weak_count(&val.shared_state))));
    });

    g.bench_function("to_arc", |b| {
        b.iter(|| black_box(Arc::weak_count(&TARGET.to_arc().shared_state)));
    });

    // For comparison, we also include a thread_local_arc! LazyCell.
    g.bench_function("vs_std_thread_local", |b| {
        b.iter(|| {
            TEST_SUBJECT_THREAD_LOCAL.with(|local| {
                black_box(Arc::weak_count(&local.shared_state));
            });
        });
    });

    // For comparison, we also include a global LazyLock.
    g.bench_function("vs_static_lazy_lock", |b| {
        b.iter(|| {
            black_box(Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state));
        });
    });

    g.finish();

    let mut g = c.benchmark_group("thread_local_arc::access_multi_threaded");

    g.bench_function("with", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(TARGET.with(|val| Arc::weak_count(&val.shared_state)));
                },
            )
        });
    });

    g.bench_function("to_arc", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(Arc::weak_count(&TARGET.to_arc().shared_state));
                },
            )
        });
    });

    g.bench_function("vs_std_thread_local", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    TEST_SUBJECT_THREAD_LOCAL.with(|local| {
                        black_box(Arc::weak_count(&local.shared_state));
                    });
                },
            )
        });
    });

    g.bench_function("vs_static_lazy_lock", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(Arc::weak_count(&TEST_SUBJECT_GLOBAL.shared_state));
                },
            )
        });
    });

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
