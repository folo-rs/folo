//! Basic operations on the `PerThread` wrapper type, used directly for dynamic storage.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::{
    cell::Cell,
    hint::black_box,
    sync::{
        Arc, LazyLock,
        atomic::{self, AtomicUsize},
    },
};

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use linked::PerThread;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[expect(
    dead_code,
    reason = "We do not care about using all the fields but we want to pay the price of initializing them"
)]
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

fn entrypoint(c: &mut Criterion) {
    per_thread(c);
    thread_local(c);
    thread_local_multithreaded(c);
    thread_local_access(c);
}

fn per_thread(c: &mut Criterion) {
    let mut g = c.benchmark_group("per_thread::PerThread");

    g.bench_function("new", |b| {
        b.iter_batched(
            || (),
            |()| black_box(PerThread::new(TestSubject::new())),
            BatchSize::SmallInput,
        )
    });

    let per_thread = PerThread::new(TestSubject::new());

    g.bench_function("clone", |b| {
        b.iter_batched(
            || (),
            |()| black_box(per_thread.clone()),
            BatchSize::SmallInput,
        )
    });

    g.finish();
}

fn thread_local(c: &mut Criterion) {
    let mut g = c.benchmark_group("per_thread::ThreadLocal");

    let per_thread = PerThread::new(TestSubject::new());

    g.bench_function("new_single", |b| {
        b.iter_batched(
            || (),
            |()| black_box(per_thread.local()),
            BatchSize::SmallInput,
        )
    });

    {
        // We keep one ThreadLocal here so the ones we create in iterations are not the only ones.
        let _first = per_thread.local();

        g.bench_function("new_not_single", |b| {
            b.iter_batched(
                || (),
                |()| black_box(per_thread.local()),
                BatchSize::SmallInput,
            )
        });
    }

    {
        // This is the one we clone.
        let first = per_thread.local();

        g.bench_function("clone", |b| {
            b.iter_batched(|| (), |()| black_box(first.clone()), BatchSize::SmallInput)
        });
    }

    g.finish();
}

fn thread_local_multithreaded(c: &mut Criterion) {
    let thread_pool = ThreadPool::all();

    let mut g = c.benchmark_group("per_thread::ThreadLocalMultithreaded");

    let per_thread = PerThread::new(TestSubject::new());

    g.bench_function("new_single", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(&thread_pool, iters, || (), {
                let per_thread = per_thread.clone();
                move |()| {
                    black_box(per_thread.local());
                }
            })
        });
    });

    g.bench_function("new_not_single", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                {
                    let per_thread = per_thread.clone();
                    move || {
                        // This is the first one, we just keep it around for all the iterations.
                        per_thread.local()
                    }
                },
                {
                    let per_thread = per_thread.clone();
                    move |_| {
                        black_box(per_thread.local());
                    }
                },
            )
        });
    });

    g.bench_function("clone", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                {
                    let per_thread = per_thread.clone();
                    move || {
                        // This is the first one that we clone.
                        per_thread.local()
                    }
                },
                {
                    move |first| {
                        black_box(first.clone());
                    }
                },
            )
        });
    });

    g.finish();
}

fn thread_local_access(c: &mut Criterion) {
    let mut g = c.benchmark_group("per_thread::ThreadLocalAccess");

    let per_thread = PerThread::new(TestSubject::new());

    g.bench_function("deref", |b| {
        let local = per_thread.local();

        b.iter(|| {
            black_box(local.local_state.get());
        });
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

    let thread_pool = ThreadPool::all();

    // For comparison, also the LazyLock in multithreaded mode, as all the other
    // ones are thread-local and have no MT overhead but this may have overhead.
    g.bench_function("vs_static_lazy_lock_mt", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
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
