#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use std::sync::mpsc;
use threadpool::ThreadPool;
use vicinal::Pool;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    // Pin the main thread to a single processor to eliminate OS migration noise.
    if let Some(processors) = ProcessorSet::builder().take(nz!(1)) {
        processors.pin_current_thread_to();
    }

    let allocs = AllocSession::new();
    let times = TimeSession::new();

    let mut g = c.benchmark_group("spawn");

    let spawn_single_alloc = allocs.operation("spawn_single");
    let spawn_single_time = times.operation("spawn_single");

    g.bench_function("spawn_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = spawn_single_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_single_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                let handle = scheduler.spawn(|| black_box(42));
                black_box(block_on(handle));
            }

            start.elapsed()
        });
    });

    let spawn_100_alloc = allocs.operation("spawn_100");
    let spawn_100_time = times.operation("spawn_100");

    g.bench_function("spawn_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = spawn_100_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_100_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(scheduler.spawn(move || black_box(i)));
                }

                #[allow(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for handle in handles.drain(..) {
                    black_box(block_on(handle));
                }
            }

            start.elapsed()
        });
    });

    let spawn_urgent_single_alloc = allocs.operation("spawn_urgent_single");
    let spawn_urgent_single_time = times.operation("spawn_urgent_single");

    g.bench_function("spawn_urgent_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = spawn_urgent_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_urgent_single_time
                .measure_thread()
                .iterations(iterations);

            for _ in 0..iterations {
                let handle = scheduler.spawn_urgent(|| black_box(42));
                black_box(block_on(handle));
            }

            start.elapsed()
        });
    });

    let spawn_urgent_100_alloc = allocs.operation("spawn_urgent_100");
    let spawn_urgent_100_time = times.operation("spawn_urgent_100");

    g.bench_function("spawn_urgent_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = spawn_urgent_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_urgent_100_time
                .measure_thread()
                .iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(scheduler.spawn_urgent(move || black_box(i)));
                }

                #[allow(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for handle in handles.drain(..) {
                    black_box(block_on(handle));
                }
            }

            start.elapsed()
        });
    });

    let spawn_only_alloc = allocs.operation("spawn_only");
    let spawn_only_time = times.operation("spawn_only");

    g.bench_function("spawn_only", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let capacity = usize::try_from(iterations).expect("iterations fits in usize");
            let mut handles = Vec::with_capacity(capacity);

            let start = Instant::now();
            let _alloc_span = spawn_only_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_only_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                handles.push(scheduler.spawn(|| black_box(42)));
            }

            let elapsed = start.elapsed();

            // Await outside measured section to avoid measuring await overhead.
            for handle in handles {
                black_box(block_on(handle));
            }

            elapsed
        });
    });

    let thread_single_alloc = allocs.operation("thread_single");
    let thread_single_time = times.operation("thread_single");

    g.bench_function("thread_single", |b| {
        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = thread_single_alloc.measure_process().iterations(iterations);
            let _time_span = thread_single_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                let handle = std::thread::spawn(|| black_box(42));
                black_box(handle.join().unwrap());
            }

            start.elapsed()
        });
    });

    let thread_100_alloc = allocs.operation("thread_100");
    let thread_100_time = times.operation("thread_100");

    g.bench_function("thread_100", |b| {
        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = thread_100_alloc.measure_process().iterations(iterations);
            let _time_span = thread_100_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(std::thread::spawn(move || black_box(i)));
                }

                #[allow(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for handle in handles.drain(..) {
                    black_box(handle.join().unwrap());
                }
            }

            start.elapsed()
        });
    });

    let threadpool_single_alloc = allocs.operation("threadpool_single");
    let threadpool_single_time = times.operation("threadpool_single");

    g.bench_function("threadpool_single", |b| {
        let pool = ThreadPool::new(2);

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = threadpool_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_single_time
                .measure_thread()
                .iterations(iterations);

            for _ in 0..iterations {
                let (tx, rx) = mpsc::channel();
                pool.execute(move || {
                    black_box(42);
                    tx.send(()).unwrap();
                });
                let _: () = rx.recv().unwrap();
                black_box(());
            }

            start.elapsed()
        });
    });

    let threadpool_100_alloc = allocs.operation("threadpool_100");
    let threadpool_100_time = times.operation("threadpool_100");

    g.bench_function("threadpool_100", |b| {
        let pool = ThreadPool::new(2);

        b.iter_custom(|iterations| {
            let mut rxs = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = threadpool_100_alloc.measure_process().iterations(iterations);
            let _time_span = threadpool_100_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    let (tx, rx) = mpsc::channel();
                    rxs.push(rx);
                    pool.execute(move || {
                        black_box(i);
                        tx.send(()).unwrap();
                    });
                }

                #[allow(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for rx in rxs.drain(..) {
                    let _: () = rx.recv().unwrap();
                    black_box(());
                }
            }

            start.elapsed()
        });
    });

    g.finish();

    allocs.print_to_stdout();
    times.print_to_stdout();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
