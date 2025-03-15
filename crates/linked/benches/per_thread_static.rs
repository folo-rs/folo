use std::{
    cell::Cell,
    hint::black_box,
    sync::{
        Arc, LazyLock,
        atomic::{self, AtomicUsize},
    },
};

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

// We do not necessarily care about using all the fields, but we want to pay the price of initializing them.
#[allow(dead_code)]
#[linked::object]
struct TestSubject {
    local_state: Cell<usize>,
    shared_state: Arc<AtomicUsize>,
}

impl TestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));

        linked::new!(Self {
            local_state: Cell::new(0),
            shared_state: Arc::clone(&shared_state),
        })
    }
}

linked::instance_per_thread!(static TARGET: TestSubject = TestSubject::new());

fn entrypoint(c: &mut Criterion) {
    let thread_pool = ThreadPool::all();

    let mut g = c.benchmark_group("per_thread_static::access_single_threaded");

    g.bench_function("with", |b| {
        b.iter(|| black_box(TARGET.with(|val| val.local_state.get())));
    });

    g.bench_function("to_rc", |b| {
        b.iter(|| black_box(TARGET.to_rc().local_state.get()));
    });

    // For comparison, we also include a thread_local! LazyCell.
    g.bench_function("vs_std_thread_local", |b| {
        b.iter(|| {
            TEST_SUBJECT_THREAD_LOCAL.with(|local| {
                black_box(local.local_state.get());
            });
        });
    });

    // For comparison, we also include a global LazyLock.
    g.bench_function("vs_static_lazy_lock", |b| {
        b.iter(|| {
            black_box(
                TEST_SUBJECT_GLOBAL
                    .local_state
                    .load(atomic::Ordering::Relaxed),
            );
        });
    });

    g.finish();

    let mut g = c.benchmark_group("per_thread_static::access_multi_threaded");

    g.bench_function("with", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |_| {
                    black_box(TARGET.with(|val| val.local_state.get()));
                },
            )
        });
    });

    g.bench_function("to_rc", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |_| {
                    black_box(TARGET.to_rc().local_state.get());
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
                |_| {
                    TEST_SUBJECT_THREAD_LOCAL.with(|local| {
                        black_box(local.local_state.get());
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
                |_| {
                    black_box(
                        TEST_SUBJECT_GLOBAL
                            .local_state
                            .load(atomic::Ordering::Relaxed),
                    );
                },
            )
        });
    });

    g.finish();
}

// Rough equivalent of the above TestSubject, to compare non-linked object via LazyLock.
struct ComparisonTestSubject {
    local_state: AtomicUsize,
    _shared_state: Arc<AtomicUsize>,
}

impl ComparisonTestSubject {
    fn new() -> Self {
        let shared_state = Arc::new(AtomicUsize::new(0));

        ComparisonTestSubject {
            local_state: AtomicUsize::new(0),
            _shared_state: shared_state,
        }
    }
}

thread_local! {
    static TEST_SUBJECT_THREAD_LOCAL: TestSubject = TestSubject::new();
}

static TEST_SUBJECT_GLOBAL: LazyLock<ComparisonTestSubject> =
    LazyLock::new(ComparisonTestSubject::new);
