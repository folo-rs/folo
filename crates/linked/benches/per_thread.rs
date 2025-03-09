use std::{
    cell::Cell,
    hint::black_box,
    sync::{Arc, Barrier, atomic::AtomicUsize},
    time::{Duration, Instant},
};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use linked::PerThread;
use many_cpus::ProcessorSet;

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
    let mut g = c.benchmark_group("PerThread");

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
    let mut g = c.benchmark_group("ThreadLocal");

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
    let mut g = c.benchmark_group("ThreadLocalMultithreaded");

    let per_thread = PerThread::new(TestSubject::new());

    g.bench_function("new_single", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(iters, || (), {
                let per_thread = per_thread.clone();
                move |_| {
                    black_box(per_thread.local());
                }
            })
        });
    });

    g.bench_function("new_not_single", |b| {
        b.iter_custom(|iters| {
            bench_on_every_processor(
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
            bench_on_every_processor(
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

fn bench_on_every_processor<P, D, F>(iters: u64, prepare_fn: P, iter_fn: F) -> Duration
where
    P: Fn() -> D + Send + Clone + 'static,
    F: Fn(&D) + Send + Clone + 'static,
{
    let processors = ProcessorSet::all();

    // All threads will wait on this before starting, so they start together.
    let barrier = Arc::new(Barrier::new(processors.len()));

    let threads = processors.spawn_threads({
        let barrier = barrier.clone();
        move |_| {
            let data = prepare_fn();

            barrier.wait();

            let start = Instant::now();

            for _ in 0..iters {
                iter_fn(&data);
            }

            let elapsed = start.elapsed();

            drop(data);

            elapsed
        }
    });

    let mut total_elapsed_nanos = 0;

    let thread_count = threads.len();

    for thread in threads {
        let elapsed = thread.join().unwrap();
        total_elapsed_nanos += elapsed.as_nanos();
    }

    Duration::from_nanos((total_elapsed_nanos / thread_count as u128) as u64)
}

fn thread_local_access(c: &mut Criterion) {
    let mut g = c.benchmark_group("ThreadLocalAccess");

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
