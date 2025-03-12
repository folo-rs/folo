use std::{
    cell::Cell,
    hint::black_box,
    sync::{Arc, atomic::AtomicUsize},
};

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use linked::PerThread;

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
            |_| black_box(PerThread::new(TestSubject::new())),
            BatchSize::SmallInput,
        )
    });

    let per_thread = PerThread::new(TestSubject::new());

    g.bench_function("clone", |b| {
        b.iter_batched(
            || (),
            |_| black_box(per_thread.clone()),
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
            |_| black_box(per_thread.local()),
            BatchSize::SmallInput,
        )
    });

    {
        // We keep one ThreadLocal here so the ones we create in iterations are not the only ones.
        let _first = per_thread.local();

        g.bench_function("new_not_single", |b| {
            b.iter_batched(
                || (),
                |_| black_box(per_thread.local()),
                BatchSize::SmallInput,
            )
        });
    }

    {
        // This is the one we clone.
        let first = per_thread.local();

        g.bench_function("clone", |b| {
            b.iter_batched(|| (), |_| black_box(first.clone()), BatchSize::SmallInput)
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
                move |_| {
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
        b.iter_batched(
            || per_thread.local(),
            |local| {
                black_box(local.local_state.get());
                local
            },
            BatchSize::SmallInput,
        )
    });

    g.finish();
}
