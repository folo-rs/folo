#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::thread;
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use events_once::EventPool;
use futures::executor::block_on;
use many_cpus::SystemHardware;
use new_zealand::nz;
use threadpool::ThreadPool;
use vicinal::Pool;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// A small, deterministic, CPU-only stand-in for a real task body.
///
/// These benchmarks measure the overhead of *dispatching* work onto a worker,
/// so the task body must be cheap and predictable. It performs no syscalls (see
/// `docs/benchmarks.md`), which would otherwise inject kernel and filesystem
/// noise unrelated to the scheduling under test. A handful of LCG steps seeded
/// with `black_box` gives a fixed cost the optimizer cannot fold away.
fn simulate_work() -> u32 {
    let mut value = black_box(42_u32);
    for _ in 0..32 {
        value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    value
}

fn entrypoint(c: &mut Criterion) {
    // Pin the main thread to a single processor to eliminate OS migration impact.
    let one_processor = SystemHardware::current()
        .processors()
        .to_builder()
        .take(nz!(1))
        .unwrap();
    one_processor.pin_current_thread_to();

    let allocs = AllocSession::new();
    let times = TimeSession::new();

    let mut g = c.benchmark_group("vicinal/spawn");

    let spawn_single_alloc = allocs.operation("vicinal/spawn/spawn_single");
    let spawn_single_time = times.operation("vicinal/spawn/spawn_single");

    g.bench_function("spawn_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let _alloc_span = spawn_single_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_single_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let handle = scheduler.spawn(|| black_box(simulate_work()));
                black_box(block_on(handle));
            }

            start.elapsed()
        });
    });

    let spawn_100_alloc = allocs.operation("vicinal/spawn/spawn_100");
    let spawn_100_time = times.operation("vicinal/spawn/spawn_100");

    g.bench_function("spawn_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let _alloc_span = spawn_100_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..100 {
                    handles.push(scheduler.spawn(move || black_box(simulate_work())));
                }

                #[expect(
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

    let spawn_urgent_single_alloc = allocs.operation("vicinal/spawn/spawn_urgent_single");
    let spawn_urgent_single_time = times.operation("vicinal/spawn/spawn_urgent_single");

    g.bench_function("spawn_urgent_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let _alloc_span = spawn_urgent_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_urgent_single_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let handle = scheduler.spawn_urgent(|| black_box(simulate_work()));
                black_box(block_on(handle));
            }

            start.elapsed()
        });
    });

    let spawn_urgent_100_alloc = allocs.operation("vicinal/spawn/spawn_urgent_100");
    let spawn_urgent_100_time = times.operation("vicinal/spawn/spawn_urgent_100");

    g.bench_function("spawn_urgent_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let _alloc_span = spawn_urgent_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_urgent_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..100 {
                    handles.push(scheduler.spawn_urgent(move || black_box(simulate_work())));
                }

                #[expect(
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

    let thread_single_alloc = allocs.operation("vicinal/spawn/thread_single");
    let thread_single_time = times.operation("vicinal/spawn/thread_single");

    g.bench_function("thread_single", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = thread_single_alloc.measure_process().iterations(iterations);
            let _time_span = thread_single_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let handle = thread::spawn(|| black_box(simulate_work()));
                black_box(handle.join().unwrap());
            }

            start.elapsed()
        });
    });

    let thread_100_alloc = allocs.operation("vicinal/spawn/thread_100");
    let thread_100_time = times.operation("vicinal/spawn/thread_100");

    g.bench_function("thread_100", |b| {
        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let _alloc_span = thread_100_alloc.measure_process().iterations(iterations);
            let _time_span = thread_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..100 {
                    handles.push(thread::spawn(move || black_box(simulate_work())));
                }

                #[expect(
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

    let threadpool_single_alloc = allocs.operation("vicinal/spawn/threadpool_single");
    let threadpool_single_time = times.operation("vicinal/spawn/threadpool_single");

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
            let _alloc_span = threadpool_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_single_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

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

    let threadpool_100_alloc = allocs.operation("vicinal/spawn/threadpool_100");
    let threadpool_100_time = times.operation("vicinal/spawn/threadpool_100");

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

            let _alloc_span = threadpool_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..100 {
                    let (tx, rx) = event_pool.rent();
                    rxs.push(rx);
                    pool.execute(move || {
                        tx.send(black_box(simulate_work()));
                    });
                }

                #[expect(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for rx in rxs.drain(..) {
                    block_on(rx).unwrap();
                }
            }

            start.elapsed()
        });
    });

    g.finish();

    // Benchmark group for fire-and-forget variants with EventPool<()> completion signaling.
    // This group includes both regular spawn (with event) and spawn_and_forget for fair comparison.
    let mut g = c.benchmark_group("vicinal/fire_and_forget");

    let spawn_with_event_single_alloc =
        allocs.operation("vicinal/fire_and_forget/spawn_with_event_single");
    let spawn_with_event_single_time =
        times.operation("vicinal/fire_and_forget/spawn_with_event_single");

    g.bench_function("spawn_with_event_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        b.iter_custom(|iterations| {
            let _alloc_span = spawn_with_event_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_with_event_single_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let (tx, rx) = event_pool.rent();
                let _handle = scheduler.spawn(move || {
                    let result = black_box(simulate_work());
                    tx.send(());
                    result
                });
                block_on(rx).unwrap();
            }

            start.elapsed()
        });
    });

    let spawn_with_event_100_alloc =
        allocs.operation("vicinal/fire_and_forget/spawn_with_event_100");
    let spawn_with_event_100_time = times.operation("vicinal/fire_and_forget/spawn_with_event_100");

    g.bench_function("spawn_with_event_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        b.iter_custom(|iterations| {
            let mut rxs = Vec::with_capacity(100);

            let _alloc_span = spawn_with_event_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_with_event_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..100 {
                    let (tx, rx) = event_pool.rent();
                    rxs.push(rx);
                    let _handle = scheduler.spawn(move || {
                        let result = black_box(simulate_work());
                        tx.send(());
                        result
                    });
                }

                #[expect(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for rx in rxs.drain(..) {
                    block_on(rx).unwrap();
                }
            }

            start.elapsed()
        });
    });

    let spawn_and_forget_single_alloc =
        allocs.operation("vicinal/fire_and_forget/spawn_and_forget_single");
    let spawn_and_forget_single_time =
        times.operation("vicinal/fire_and_forget/spawn_and_forget_single");

    g.bench_function("spawn_and_forget_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        b.iter_custom(|iterations| {
            let _alloc_span = spawn_and_forget_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_and_forget_single_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let (tx, rx) = event_pool.rent();
                scheduler.spawn_and_forget(move || {
                    black_box(simulate_work());
                    tx.send(());
                });
                block_on(rx).unwrap();
            }

            start.elapsed()
        });
    });

    let spawn_and_forget_100_alloc =
        allocs.operation("vicinal/fire_and_forget/spawn_and_forget_100");
    let spawn_and_forget_100_time = times.operation("vicinal/fire_and_forget/spawn_and_forget_100");

    g.bench_function("spawn_and_forget_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        b.iter_custom(|iterations| {
            let mut rxs = Vec::with_capacity(100);

            let _alloc_span = spawn_and_forget_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_and_forget_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..100 {
                    let (tx, rx) = event_pool.rent();
                    rxs.push(rx);
                    scheduler.spawn_and_forget(move || {
                        black_box(simulate_work());
                        tx.send(());
                    });
                }

                #[expect(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for rx in rxs.drain(..) {
                    block_on(rx).unwrap();
                }
            }

            start.elapsed()
        });
    });

    g.finish();

    // `allocs` and `times` print their summaries and write JSON to the Cargo
    // target directory when they are dropped at the end of this function.
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
