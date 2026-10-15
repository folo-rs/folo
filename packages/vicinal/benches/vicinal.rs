#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::measurement::WallTime;
use criterion::{
    BatchSize, BenchmarkGroup, Criterion, SamplingMode, criterion_group, criterion_main,
};
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

/// Shares affinity and measurement sessions across initialization and dispatch comparisons.
fn entrypoint(criterion: &mut Criterion) {
    // Pin the main thread to a single processor to eliminate OS migration impact.
    let one_processor = SystemHardware::current()
        .processors()
        .to_builder()
        .take(nz!(1))
        .unwrap();
    one_processor.pin_current_thread_to();

    let allocs = AllocSession::new();
    let times = TimeSession::new();

    initialization(criterion, &one_processor);
    joined_dispatch(criterion, &allocs, &times, &one_processor);
    fire_and_forget_dispatch(criterion, &allocs, &times);
    policy_dispatch(criterion, &allocs, &times);
    region_contention(criterion, &one_processor);

    // The sessions emit their summaries and JSON after every group has completed.
}

/// Separates pool construction from the first joined task on a fresh pool.
///
/// Both modes use the same pinned caller and worker capacity. The pool itself supplies
/// the worker threads; an additional harness pool would not contribute to either measurement.
fn initialization(criterion: &mut Criterion, one_processor: &ProcessorSet) {
    let mut group = criterion.benchmark_group("vicinal/initialization");
    // OS worker startup and teardown exceed the default window with linear sampling's
    // minimum triangular iteration schedule. Flat sampling retains the full sample count.
    group.sampling_mode(SamplingMode::Flat);

    for (name, eager) in [("lazy", false), ("eager", true)] {
        let builder = || {
            // Fix the destination capacity to match the dispatch comparison, independently
            // of future changes to the library default. No application initializer is installed.
            let builder = Pool::builder().worker_policy(WorkerPolicy::per_processor(nz!(2_u32)));
            if eager {
                builder.eager(one_processor.clone())
            } else {
                builder
            }
        };

        group.bench_function(format!("create/{name}"), |b| {
            b.iter_batched(
                &builder,
                |builder| black_box(builder.build()),
                // Drop and join each returned pool outside measurement before creating another.
                // Larger batches would leave idle worker pools competing with later iterations.
                BatchSize::PerIteration,
            );
        });

        group.bench_function(format!("first_task/{name}"), |b| {
            b.iter_batched_ref(
                || {
                    let pool = builder().build();
                    let scheduler = pool.scheduler();
                    // Do not submit warmup work: task slots and result channels must be cold.
                    (pool, scheduler)
                },
                |(_, scheduler)| {
                    let handle = scheduler.spawn(|| black_box(simulate_work()));
                    black_box(block_on(handle).unwrap());
                },
                // Keep pool creation, scheduler acquisition and teardown outside measurement.
                // Each iteration owns a fresh pool, including during Criterion's warmup.
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

/// Keeps joined task dispatch and its thread-based baselines in one comparison group.
fn joined_dispatch(
    criterion: &mut Criterion,
    allocs: &AllocSession,
    times: &TimeSession,
    one_processor: &ProcessorSet,
) {
    let mut group = criterion.benchmark_group("vicinal/spawn");

    spawn_regular(&mut group, allocs, times);
    spawn_urgent(&mut group, allocs, times);
    spawn_threads(&mut group, allocs, times);
    spawn_threadpool(&mut group, allocs, times, one_processor);

    group.finish();
}

/// Measures joined regular tasks individually and as a queued batch.
fn spawn_regular(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &AllocSession,
    times: &TimeSession,
) {
    let spawn_single_alloc = allocs.operation("vicinal/spawn/spawn_single");
    let spawn_single_time = times.operation("vicinal/spawn/spawn_single");

    group.bench_function("spawn_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let _alloc_span = spawn_single_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_single_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let handle = scheduler.spawn(|| black_box(simulate_work()));
                black_box(block_on(handle).unwrap());
            }

            start.elapsed()
        });
    });

    let spawn_100_alloc = allocs.operation("vicinal/spawn/spawn_100");
    let spawn_100_time = times.operation("vicinal/spawn/spawn_100");

    group.bench_function("spawn_100", |b| {
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
                    black_box(block_on(handle).unwrap());
                }
            }

            start.elapsed()
        });
    });
}

/// Applies the same dispatch workloads to the urgent queue.
fn spawn_urgent(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &AllocSession,
    times: &TimeSession,
) {
    let spawn_urgent_single_alloc = allocs.operation("vicinal/spawn/spawn_urgent_single");
    let spawn_urgent_single_time = times.operation("vicinal/spawn/spawn_urgent_single");

    group.bench_function("spawn_urgent_single", |b| {
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
                black_box(block_on(handle).unwrap());
            }

            start.elapsed()
        });
    });

    let spawn_urgent_100_alloc = allocs.operation("vicinal/spawn/spawn_urgent_100");
    let spawn_urgent_100_time = times.operation("vicinal/spawn/spawn_urgent_100");

    group.bench_function("spawn_urgent_100", |b| {
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
                    black_box(block_on(handle).unwrap());
                }
            }

            start.elapsed()
        });
    });
}

/// Provides the cost baseline of creating an operating-system thread for every task.
fn spawn_threads(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &AllocSession,
    times: &TimeSession,
) {
    let thread_single_alloc = allocs.operation("vicinal/spawn/thread_single");
    let thread_single_time = times.operation("vicinal/spawn/thread_single");

    group.bench_function("thread_single", |b| {
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

    group.bench_function("thread_100", |b| {
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
}

/// Compares an external pool at the same worker count and processor placement.
fn spawn_threadpool(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &AllocSession,
    times: &TimeSession,
    one_processor: &ProcessorSet,
) {
    let threadpool_single_alloc = allocs.operation("vicinal/spawn/threadpool_single");
    let threadpool_single_time = times.operation("vicinal/spawn/threadpool_single");

    group.bench_function("threadpool_single", |b| {
        let pool = pinned_threadpool(one_processor);
        let event_pool = EventPool::<u32>::new();

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

    group.bench_function("threadpool_100", |b| {
        let pool = pinned_threadpool(one_processor);
        let event_pool = EventPool::<u32>::new();

        b.iter_custom(|iterations| {
            let mut receivers = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = threadpool_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = threadpool_100_time.measure_process().iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
                    let (sender, receiver) = event_pool.rent();
                    receivers.push(receiver);
                    pool.execute(move || {
                        sender.send(black_box(simulate_work()));
                    });
                }

                #[expect(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for receiver in receivers.drain(..) {
                    block_on(receiver).unwrap();
                }
            }

            start.elapsed()
        });
    });
}

/// Prepares the comparison pool outside measurement, using the same placement for both cases.
fn pinned_threadpool(one_processor: &ProcessorSet) -> ThreadPool {
    let pool = ThreadPool::new(THREADPOOL_WORKER_COUNT);

    // Submit more pinning callbacks than workers because the pool does not guarantee
    // which worker runs each callback.
    for _ in 0..THREADPOOL_PIN_TASK_COUNT {
        pool.execute({
            let one_processor = one_processor.clone();
            move || one_processor.pin_current_thread_to()
        });
    }
    pool.join();

    pool
}

/// Uses identical event-channel completion signaling for joined and fire-and-forget dispatch.
fn fire_and_forget_dispatch(criterion: &mut Criterion, allocs: &AllocSession, times: &TimeSession) {
    let mut group = criterion.benchmark_group("vicinal/fire_and_forget");

    spawn_with_event(&mut group, allocs, times);
    spawn_and_forget(&mut group, allocs, times);

    group.finish();
}

/// Measures joined submission while an explicit event, rather than its handle, signals completion.
fn spawn_with_event(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &AllocSession,
    times: &TimeSession,
) {
    let spawn_with_event_single_alloc =
        allocs.operation("vicinal/fire_and_forget/spawn_with_event_single");
    let spawn_with_event_single_time =
        times.operation("vicinal/fire_and_forget/spawn_with_event_single");

    group.bench_function("spawn_with_event_single", |b| {
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

    group.bench_function("spawn_with_event_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        b.iter_custom(|iterations| {
            let mut receivers = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = spawn_with_event_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_with_event_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
                    let (sender, receiver) = event_pool.rent();
                    receivers.push(receiver);
                    let _handle = scheduler.spawn(move || {
                        let result = black_box(simulate_work());
                        sender.send(());
                        result
                    });
                }

                #[expect(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for receiver in receivers.drain(..) {
                    block_on(receiver).unwrap();
                }
            }

            start.elapsed()
        });
    });
}

/// Measures fire-and-forget dispatch with the same completion signals as the joined comparison.
fn spawn_and_forget(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &AllocSession,
    times: &TimeSession,
) {
    let spawn_and_forget_single_alloc =
        allocs.operation("vicinal/fire_and_forget/spawn_and_forget_single");
    let spawn_and_forget_single_time =
        times.operation("vicinal/fire_and_forget/spawn_and_forget_single");

    group.bench_function("spawn_and_forget_single", |b| {
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

    group.bench_function("spawn_and_forget_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        b.iter_custom(|iterations| {
            let mut receivers = Vec::with_capacity(TASK_BATCH_SIZE);

            let _alloc_span = spawn_and_forget_100_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_and_forget_100_time
                .measure_process()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                for _ in 0..TASK_BATCH_SIZE {
                    let (sender, receiver) = event_pool.rent();
                    receivers.push(receiver);
                    scheduler.spawn_and_forget(move || {
                        black_box(simulate_work());
                        sender.send(());
                    });
                }

                #[expect(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for receiver in receivers.drain(..) {
                    block_on(receiver).unwrap();
                }
            }

            start.elapsed()
        });
    });
}

/// Compares locality routing with one submission and a short queued batch.
fn policy_dispatch(criterion: &mut Criterion, allocs: &AllocSession, times: &TimeSession) {
    let mut group = criterion.benchmark_group("vicinal/policy_dispatch");

    // Keep equal destination capacity at a fixed comparison baseline that supports overlapping
    // tasks. This benchmark count does not track future changes to the pool's default.
    for (policy_name, policy) in [
        ("processor", WorkerPolicy::per_processor(nz!(2_u32))),
        ("region", WorkerPolicy::per_memory_region(nz!(2_u32))),
    ] {
        // The low case isolates dispatch; the high case exercises shared queue/slot reuse.
        for batch_size in [1, TASK_BATCH_SIZE] {
            let name = format!("{policy_name}/{batch_size}");
            let allocation = allocs.operation(format!("vicinal/policy_dispatch/{name}"));
            let time = times.operation(format!("vicinal/policy_dispatch/{name}"));

            group.bench_function(name, |b| {
                let pool = Pool::builder().worker_policy(policy.clone()).build();
                let scheduler = pool.scheduler();
                block_on(scheduler.spawn(|| {})).unwrap();
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
                            black_box(block_on(handle).unwrap());
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
fn region_contention(criterion: &mut Criterion, caller: &ProcessorSet) {
    let region_id = caller.iter().next().unwrap().memory_region_id();
    let region = SystemHardware::current()
        .all_processors()
        .to_builder()
        .filter(|p| p.memory_region_id() == region_id)
        .take_all()
        .unwrap();
    let mut group = criterion.benchmark_group("vicinal/region_contention");

    // A single submitter is the control; competing submitters expose regional queue contention.
    for callers in [1_usize, 2] {
        group.bench_function(format!("callers/{callers}"), |b| {
            // Use the same fixed destination capacity as policy_dispatch so contention is
            // measured without changing the amount of worker parallelism.
            let pool = Pool::builder()
                .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
                .build();
            let scheduler = pool.scheduler();

            // Only producers submit, including during setup, so the controller cannot expand
            // worker affinity beyond this case's intended caller processors.
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

                                // Membership expansion and initial slot/channel allocation are
                                // setup, not steady-state dispatch contention.
                                block_on(scheduler.spawn(|| {})).unwrap();
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
                                        black_box(block_on(handle).unwrap());
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
