#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use events_once::EventPool;
use futures::executor::block_on;
use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;
use testing::DefaultAllocator;
use threadpool::ThreadPool;
use vicinal::{Pool, WorkerPolicy};

#[global_allocator]
static ALLOCATOR: Allocator<DefaultAllocator> = Allocator::new(DefaultAllocator);

/// Keeps the low and high cases distinct without making each sample excessively long.
const TASK_BATCH_SIZE: usize = 100;

/// Matches the number of workers Vicinal assigns to each processor.
const THREADPOOL_WORKER_COUNT: usize = 2;

/// Makes it likely that every comparison-pool worker executes the pinning callback.
const THREADPOOL_PIN_TASK_COUNT: usize = 30;

/// Provides enough arithmetic to keep the task body observable without masking dispatch costs.
const WORK_STEP_COUNT: usize = 32;

/// A deterministic input prevents the optimizer from replacing the task body with a constant.
const WORK_SEED: u32 = 42;

/// Standard LCG parameters provide inexpensive deterministic arithmetic.
const LCG_MULTIPLIER: u32 = 1_664_525;
const LCG_INCREMENT: u32 = 1_013_904_223;

/// A small, deterministic, CPU-only stand-in for a real task body.
///
/// These benchmarks measure the overhead of *dispatching* work onto a worker,
/// so the task body must be cheap and predictable. It performs no syscalls (see
/// `docs/benchmarks.md`), which would otherwise inject kernel and filesystem
/// noise unrelated to the scheduling under test. A handful of LCG steps seeded
/// with `black_box` gives a fixed cost the optimizer cannot fold away.
fn simulate_work() -> u32 {
    let mut value = black_box(WORK_SEED);
    for _ in 0..WORK_STEP_COUNT {
        value = value
            .wrapping_mul(LCG_MULTIPLIER)
            .wrapping_add(LCG_INCREMENT);
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
            let mut handles = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = spawn_100_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
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
            let mut handles = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = spawn_urgent_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_urgent_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
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
            let mut handles = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = thread_100_alloc.measure_process().iterations(iterations);
            let _time_span = thread_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
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
        // Match the number of workers Vicinal assigns to the selected processor.
        let pool = ThreadPool::new(THREADPOOL_WORKER_COUNT);
        let event_pool = EventPool::<u32>::new();

        // Submit more pinning callbacks than workers because the pool does not guarantee
        // which worker runs each callback.
        for _ in 0..THREADPOOL_PIN_TASK_COUNT {
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
        // Match the number of workers Vicinal assigns to the selected processor.
        let pool = ThreadPool::new(THREADPOOL_WORKER_COUNT);
        let event_pool = EventPool::<u32>::new();

        // Submit more pinning callbacks than workers because the pool does not guarantee
        // which worker runs each callback.
        for _ in 0..THREADPOOL_PIN_TASK_COUNT {
            pool.execute({
                let one_processor = one_processor.clone();
                move || {
                    one_processor.pin_current_thread_to();
                }
            });
        }

        pool.join();

        b.iter_custom(|iterations| {
            let mut rxs = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = threadpool_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
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
            let mut rxs = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = spawn_with_event_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_with_event_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
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
            let mut rxs = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = spawn_and_forget_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_and_forget_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
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

    policy_dispatch(c, &allocs, &times);
    region_contention(c, &one_processor);

    // `allocs` and `times` print their summaries and write JSON to the Cargo
    // target directory when they are dropped at the end of this function.
}

/// Compares locality routing with one submission and a short queued batch.
fn policy_dispatch(c: &mut Criterion, allocs: &AllocSession, times: &TimeSession) {
    let mut group = c.benchmark_group("vicinal/policy_dispatch");
    for (policy_name, policy) in [
        ("processor", WorkerPolicy::fixed_per_processor(nz!(2_u32))),
        ("region", WorkerPolicy::fixed_per_memory_region(nz!(2_u32))),
    ] {
        // The low case isolates dispatch; the high case exercises shared queue/slot reuse.
        for batch_size in [1, TASK_BATCH_SIZE] {
            let name = format!("{policy_name}/{batch_size}");
            let allocation = allocs.operation(format!("vicinal/policy_dispatch/{name}"));
            let time = times.operation(format!("vicinal/policy_dispatch/{name}"));
            group.bench_function(name, |b| {
                let pool = Pool::builder().worker_policy(policy.clone()).build();
                let scheduler = pool.scheduler();
                block_on(scheduler.spawn(|| {}));
                let mut handles = Vec::with_capacity(batch_size);
                b.iter_custom(|iterations| {
                    let _allocations = allocation.measure_process().iterations(iterations);
                    let _time = time.measure_process().iterations(iterations);
                    let start = Instant::now();
                    for _ in 0..iterations {
                        for _ in 0..batch_size {
                            handles.push(scheduler.spawn(|| black_box(simulate_work())));
                        }
                        #[expect(clippy::iter_with_drain, reason = "reuse the batch vector")]
                        for handle in handles.drain(..) {
                            black_box(block_on(handle));
                        }
                    }
                    start.elapsed()
                });
            });
        }
    }
    group.finish();
}

/// Measures competing submitters sharing a region; thread creation and affinity are untimed.
///
/// The workspace has no `bench_on_threadpool` helper. A scoped-thread harness runs both
/// caller-count cases with identical synchronization and measures only dispatch/completion.
fn region_contention(c: &mut Criterion, caller: &ProcessorSet) {
    let region_id = caller.iter().next().unwrap().memory_region_id();
    let region = SystemHardware::current()
        .all_processors()
        .to_builder()
        .filter(|p| p.memory_region_id() == region_id)
        .take_all()
        .unwrap();
    let mut group = c.benchmark_group("vicinal/region_contention");
    // A single submitter is the control; competing submitters expose regional queue contention.
    for callers in [1_usize, 2] {
        group.bench_function(format!("callers/{callers}"), |b| {
            let pool = Pool::builder()
                .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
                .build();
            let scheduler = pool.scheduler();
            block_on(scheduler.spawn(|| {}));
            b.iter_custom(|iterations| {
                thread::scope(|scope| {
                    // The controller participates in both rendezvous barriers.
                    let participants = callers.checked_add(1).unwrap();
                    let start = Arc::new(Barrier::new(participants));
                    let finish = Arc::new(Barrier::new(participants));
                    for index in 0..callers {
                        // Small/one-CPU regions still exercise competing callers on the same CPU.
                        let processor = region
                            .iter()
                            .nth(index)
                            .unwrap_or_else(|| region.processors().first())
                            .id();
                        let affinity = region
                            .to_builder()
                            .filter(|p| p.id() == processor)
                            .take_all()
                            .unwrap();
                        scope.spawn({
                            let scheduler = scheduler.clone();
                            let start = Arc::clone(&start);
                            let finish = Arc::clone(&finish);
                            move || {
                                affinity.pin_current_thread_to();
                                let mut handles = Vec::with_capacity(TASK_BATCH_SIZE);
                                // First rendezvous reports readiness; the second releases work.
                                start.wait();
                                start.wait();
                                for _ in 0..iterations {
                                    for _ in 0..TASK_BATCH_SIZE {
                                        handles
                                            .push(scheduler.spawn(|| black_box(simulate_work())));
                                    }
                                    #[expect(
                                        clippy::iter_with_drain,
                                        reason = "reuse the batch vector"
                                    )]
                                    for handle in handles.drain(..) {
                                        black_box(block_on(handle));
                                    }
                                }
                                finish.wait();
                            }
                        });
                    }
                    // All producers have prepared affinity/storage before the measured release.
                    // The symmetric barriers include the same controller handoff in both cases.
                    start.wait();
                    let started = Instant::now();
                    start.wait();
                    finish.wait();
                    started.elapsed()
                })
            });
        });
    }
    group.finish();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
