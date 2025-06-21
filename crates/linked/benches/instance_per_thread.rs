//! Basic operations on the `InstancePerThread` wrapper type, used directly for dynamic storage.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, LazyLock};

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use linked::InstancePerThread;

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

fn entrypoint(c: &mut Criterion) {
    local(c);
    local_ref(c);
    local_ref_multithreaded(c);
    local_ref_access(c);
}

fn local(c: &mut Criterion) {
    let mut g = c.benchmark_group("instance_per_thread::InstancePerThread");

    g.bench_function("new", |b| {
        b.iter_batched(
            || (),
            |()| black_box(InstancePerThread::new(TestSubject::new())),
            BatchSize::SmallInput,
        );
    });

    let per_thread = InstancePerThread::new(TestSubject::new());

    g.bench_function("clone", |b| {
        b.iter_batched(
            || (),
            |()| black_box(per_thread.clone()),
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

fn local_ref(c: &mut Criterion) {
    let mut g = c.benchmark_group("instance_per_thread::Ref");

    let per_thread = InstancePerThread::new(TestSubject::new());

    g.bench_function("new_single", |b| {
        b.iter_batched(
            || (),
            |()| black_box(per_thread.acquire()),
            BatchSize::SmallInput,
        );
    });

    {
        // We keep one InstancePerThread here so the ones we create in iterations are not the only ones.
        let _first = per_thread.acquire();

        g.bench_function("new_not_single", |b| {
            b.iter_batched(
                || (),
                |()| black_box(per_thread.acquire()),
                BatchSize::SmallInput,
            );
        });
    }

    {
        // This is the one we clone.
        let first = per_thread.acquire();

        g.bench_function("clone", |b| {
            b.iter_batched(|| (), |()| black_box(first.clone()), BatchSize::SmallInput);
        });
    }

    g.finish();
}

fn local_ref_multithreaded(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    let mut g = c.benchmark_group("instance_per_thread::Ref::multithreaded");

    let per_thread = InstancePerThread::new(TestSubject::new());

    g.bench_function("new_single", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(&thread_pool, iters, || (), {
                let per_thread = per_thread.clone();
                move |()| {
                    black_box(per_thread.acquire());
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
                        per_thread.acquire()
                    }
                },
                {
                    let per_thread = per_thread.clone();
                    move |_| {
                        black_box(per_thread.acquire());
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
                        per_thread.acquire()
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

fn local_ref_access(c: &mut Criterion) {
    let mut g = c.benchmark_group("instance_per_thread::Ref::access");

    let per_thread = InstancePerThread::new(TestSubject::new());

    g.bench_function("deref", |b| {
        let local = per_thread.acquire();

        b.iter(|| {
            black_box(Arc::weak_count(&local.shared_state));
        });
    });

    // For comparison, we also include a thread_local_rc! LazyCell.
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

    let thread_pool = ThreadPool::default();

    // For comparison, also the LazyLock in multithreaded mode, as all the other
    // ones are thread-local and have no MT overhead but this may have overhead.
    g.bench_function("vs_static_lazy_lock_mt", |b| {
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
