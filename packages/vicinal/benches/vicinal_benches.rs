#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::thread;
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use events_once::EventPool;
use futures::executor::block_on;
use many_cpus::ProcessorSet;
use new_zealand::nz;
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

    let mut g = c.benchmark_group("vicinal");

    let spawn_single_alloc = allocs.operation("spawn_single");
    let spawn_single_time = times.operation("spawn_single");

    g.bench_function("spawn_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = spawn_single_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_single_time.measure_process().iterations(iterations);

            for _ in 0..iterations {
                let handle = scheduler.spawn(|| {
                    thread::yield_now();
                    black_box(42)
                });
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
            let _time_span = spawn_100_time.measure_process().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(scheduler.spawn(move || {
                        thread::yield_now();
                        black_box(i)
                    }));
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
                .measure_process()
                .iterations(iterations);

            for _ in 0..iterations {
                let handle = scheduler.spawn_urgent(|| {
                    thread::yield_now();
                    black_box(42)
                });
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
                .measure_process()
                .iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(scheduler.spawn_urgent(move || {
                        thread::yield_now();
                        black_box(i)
                    }));
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

    let thread_single_alloc = allocs.operation("thread_single");
    let thread_single_time = times.operation("thread_single");

    g.bench_function("thread_single", |b| {
        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = thread_single_alloc.measure_process().iterations(iterations);
            let _time_span = thread_single_time.measure_process().iterations(iterations);

            for _ in 0..iterations {
                let handle = thread::spawn(|| {
                    thread::yield_now();
                    black_box(42)
                });
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
            let _time_span = thread_100_time.measure_process().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(thread::spawn(move || {
                        thread::yield_now();
                        black_box(i)
                    }));
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
        let event_pool = EventPool::<i32>::new();

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = threadpool_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_single_time
                .measure_process()
                .iterations(iterations);

            for _ in 0..iterations {
                let (tx, rx) = event_pool.rent();
                pool.execute(move || {
                    thread::yield_now();
                    tx.send(black_box(42));
                });
                black_box(block_on(rx).unwrap());
            }

            start.elapsed()
        });
    });

    let threadpool_100_alloc = allocs.operation("threadpool_100");
    let threadpool_100_time = times.operation("threadpool_100");

    g.bench_function("threadpool_100", |b| {
        let pool = ThreadPool::new(2);
        let event_pool = EventPool::<i32>::new();

        b.iter_custom(|iterations| {
            let mut rxs = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = threadpool_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_100_time.measure_process().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    let (tx, rx) = event_pool.rent();
                    rxs.push(rx);
                    pool.execute(move || {
                        thread::yield_now();
                        tx.send(black_box(i));
                    });
                }

                #[allow(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for rx in rxs.drain(..) {
                    black_box(block_on(rx).unwrap());
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
