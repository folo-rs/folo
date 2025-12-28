#![expect(missing_docs, reason = "benchmarks")]

use std::fs::File;
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

fn simulate_work() -> u32 {
    drop(File::open("Q:\\non_existent_file.txt"));
    42
}

fn entrypoint(c: &mut Criterion) {
    // Pin the main thread to a single processor to eliminate OS migration impact.
    let one_processor = ProcessorSet::builder().take(nz!(1)).unwrap();
    one_processor.pin_current_thread_to();

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
                let handle = scheduler.spawn(|| black_box(simulate_work()));
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
                for _ in 0..100 {
                    handles.push(scheduler.spawn(move || black_box(simulate_work())));
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
                let handle = scheduler.spawn_urgent(|| black_box(simulate_work()));
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
                for _ in 0..100 {
                    handles.push(scheduler.spawn_urgent(move || black_box(simulate_work())));
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
                let handle = thread::spawn(|| black_box(simulate_work()));
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
                for _ in 0..100 {
                    handles.push(thread::spawn(move || black_box(simulate_work())));
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
        // Vicinal pool defaults to 2 threads per processor, so we match.
        let pool = ThreadPool::new(2);
        let event_pool = EventPool::<u32>::new();

        // Ensure the thread pool threads are pinned to the same processor as main().
        // There is no guarantee on what thread each task gets scheduled on, so we
        // spam a whole bunch of these to make it more likely we hit both threads.
        for _ in 0..30 {
            pool.execute({
                let one_processor = one_processor.clone();
                move || {
                    one_processor.pin_current_thread_to();
                }
            });
        }

        pool.join();

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
                    tx.send(black_box(simulate_work()));
                });
                black_box(block_on(rx).unwrap());
            }

            start.elapsed()
        });
    });

    let threadpool_100_alloc = allocs.operation("threadpool_100");
    let threadpool_100_time = times.operation("threadpool_100");

    g.bench_function("threadpool_100", |b| {
        // Vicinal pool defaults to 2 threads per processor, so we match.
        let pool = ThreadPool::new(2);
        let event_pool = EventPool::<u32>::new();

        // Ensure the thread pool threads are pinned to the same processor as main().
        // There is no guarantee on what thread each task gets scheduled on, so we
        // spam a whole bunch of these to make it more likely we hit both threads.
        for _ in 0..30 {
            pool.execute({
                let one_processor = one_processor.clone();
                move || {
                    one_processor.pin_current_thread_to();
                }
            });
        }

        pool.join();

        b.iter_custom(|iterations| {
            let mut rxs = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = threadpool_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_100_time.measure_process().iterations(iterations);

            for _ in 0..iterations {
                for _ in 0..100 {
                    let (tx, rx) = event_pool.rent();
                    rxs.push(rx);
                    pool.execute(move || {
                        tx.send(black_box(simulate_work()));
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
