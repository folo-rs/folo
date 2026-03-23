//! Benchmarks comparing `events` mutex and semaphore with Tokio and
//! async-lock.
//!
//! Four benchmark groups measure different aspects:
//!
//! * **`creation`** — construction overhead for mutex and semaphore
//!   (boxed and embedded).
//! * **`round_trip`** — uncontended lock/unlock or acquire/release.
//! * **`async_poll_ready`** — lock/acquire on an available primitive,
//!   pin the future, and poll to completion in a single poll.
//! * **`many_waiters`** — register 100 waiters and release them
//!   one at a time.
//!
//! Memory allocations are tracked via `alloc_tracker` and printed to
//! stdout after all benchmarks complete.

#![allow(missing_docs, reason = "benchmark code")]
#![allow(
    clippy::let_underscore_must_use,
    reason = "poll results are intentionally discarded in benchmarks"
)]
#![allow(
    clippy::undocumented_unsafe_blocks,
    reason = "benchmark pinning has trivial safety invariants"
)]
#![allow(
    let_underscore_drop,
    reason = "guards and permits are intentionally dropped after poll"
)]
#![allow(
    unused_must_use,
    reason = "Result values from try_lock/try_acquire are consumed by black_box"
)]

use std::future::Future;
use std::hint::black_box;
use std::iter;
use std::pin::{Pin, pin};
use std::task::{Context, Waker};
use std::time::Instant;

use alloc_tracker::{Allocator, Session as AllocSession};
use asynchroniz::{
    EmbeddedLocalMutex, EmbeddedLocalSemaphore, EmbeddedMutex, EmbeddedSemaphore, LocalMutex,
    LocalSemaphore, Mutex, Semaphore,
};
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let allocs = AllocSession::new();

    creation(c, &allocs);
    round_trip(c, &allocs);
    async_poll_ready(c, &allocs);
    many_waiters(c, &allocs);

    allocs.print_to_stdout();
}

const MANY_WAITER_COUNT: usize = 100;

// -----------------------------------------------------------------------
// Creation
// -----------------------------------------------------------------------

fn creation(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("sync_creation");

    // --- Boxed ---

    let op = allocs.operation("sync_creation/events/Mutex");
    group.bench_function("events/Mutex", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(Mutex::boxed(0_u32));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("sync_creation/events/Semaphore");
    group.bench_function("events/Semaphore", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(Semaphore::boxed(1));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("sync_creation/events/LocalMutex");
    group.bench_function("events/LocalMutex", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(LocalMutex::boxed(0_u32));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("sync_creation/events/LocalSemaphore");
    group.bench_function("events/LocalSemaphore", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(LocalSemaphore::boxed(1));
            }
            start.elapsed()
        });
    });

    // --- Embedded ---

    let op = allocs.operation("sync_creation/events/embedded/Mutex");
    group.bench_function("events/embedded/Mutex", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedMutex::new(0_u32));
                let handle = unsafe { Mutex::embedded(container.as_ref()) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("sync_creation/events/embedded/Semaphore");
    group.bench_function("events/embedded/Semaphore", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedSemaphore::new(1));
                let handle = unsafe { Semaphore::embedded(container.as_ref()) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    // --- Tokio ---

    let op = allocs.operation("sync_creation/tokio/Mutex");
    group.bench_function("tokio/Mutex", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(tokio::sync::Mutex::new(0_u32));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("sync_creation/tokio/Semaphore");
    group.bench_function("tokio/Semaphore", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(tokio::sync::Semaphore::new(1));
            }
            start.elapsed()
        });
    });

    // --- async-lock ---

    let op = allocs.operation("sync_creation/async-lock/Mutex");
    group.bench_function("async-lock/Mutex", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(async_lock::Mutex::new(0_u32));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("sync_creation/async-lock/Semaphore");
    group.bench_function("async-lock/Semaphore", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(async_lock::Semaphore::new(1));
            }
            start.elapsed()
        });
    });

    group.finish();
}

// -----------------------------------------------------------------------
// Round trip (uncontended lock + unlock / acquire + release)
// -----------------------------------------------------------------------

fn round_trip(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("sync_round_trip");

    // --- events Mutex ---

    let op = allocs.operation("sync_round_trip/events/Mutex/try_lock");
    group.bench_function("events/Mutex/try_lock", |b| {
        let mutex = Mutex::boxed(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let guard = mutex.try_lock();
                black_box(guard);
            }
            start.elapsed()
        });
    });

    // --- events Semaphore ---

    let op = allocs.operation("sync_round_trip/events/Semaphore/try_acquire");
    group.bench_function("events/Semaphore/try_acquire", |b| {
        let sem = Semaphore::boxed(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let permit = sem.try_acquire();
                black_box(permit);
            }
            start.elapsed()
        });
    });

    // --- events LocalMutex ---

    let op = allocs.operation("sync_round_trip/events/LocalMutex/try_lock");
    group.bench_function("events/LocalMutex/try_lock", |b| {
        let mutex = LocalMutex::boxed(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let guard = mutex.try_lock();
                black_box(guard);
            }
            start.elapsed()
        });
    });

    // --- events LocalSemaphore ---

    let op = allocs.operation("sync_round_trip/events/LocalSemaphore/try_acquire");
    group.bench_function("events/LocalSemaphore/try_acquire", |b| {
        let sem = LocalSemaphore::boxed(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let permit = sem.try_acquire();
                black_box(permit);
            }
            start.elapsed()
        });
    });

    // --- Tokio Mutex ---

    let op = allocs.operation("sync_round_trip/tokio/Mutex/try_lock");
    group.bench_function("tokio/Mutex/try_lock", |b| {
        let mutex = tokio::sync::Mutex::new(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let guard = mutex.try_lock();
                black_box(guard);
            }
            start.elapsed()
        });
    });

    // --- Tokio Semaphore ---

    let op = allocs.operation("sync_round_trip/tokio/Semaphore/try_acquire");
    group.bench_function("tokio/Semaphore/try_acquire", |b| {
        let sem = tokio::sync::Semaphore::new(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let permit = sem.try_acquire();
                black_box(permit);
            }
            start.elapsed()
        });
    });

    // --- async-lock Mutex ---

    let op = allocs.operation("sync_round_trip/async-lock/Mutex/try_lock");
    group.bench_function("async-lock/Mutex/try_lock", |b| {
        let mutex = async_lock::Mutex::new(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let guard = mutex.try_lock();
                black_box(guard);
            }
            start.elapsed()
        });
    });

    // --- async-lock Semaphore ---

    let op = allocs.operation("sync_round_trip/async-lock/Semaphore/try_acquire");
    group.bench_function("async-lock/Semaphore/try_acquire", |b| {
        let sem = async_lock::Semaphore::new(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let permit = sem.try_acquire();
                black_box(permit);
            }
            start.elapsed()
        });
    });

    group.finish();
}

// -----------------------------------------------------------------------
// Async poll ready (lock/acquire on available primitive, single poll)
// -----------------------------------------------------------------------

fn async_poll_ready(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("sync_async_poll_ready");

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);

    // --- events Mutex ---

    let op = allocs.operation("sync_async_poll_ready/events/Mutex");
    group.bench_function("events/Mutex", |b| {
        let mutex = Mutex::boxed(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(mutex.lock());
                let _ = black_box(future.as_mut().poll(&mut cx));
                // Guard dropped here, releasing the lock.
            }
            start.elapsed()
        });
    });

    // --- events Semaphore ---

    let op = allocs.operation("sync_async_poll_ready/events/Semaphore");
    group.bench_function("events/Semaphore", |b| {
        let sem = Semaphore::boxed(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(sem.acquire());
                let _ = black_box(future.as_mut().poll(&mut cx));
                // Permit dropped here, releasing.
            }
            start.elapsed()
        });
    });

    // --- events LocalMutex ---

    let op = allocs.operation("sync_async_poll_ready/events/LocalMutex");
    group.bench_function("events/LocalMutex", |b| {
        let mutex = LocalMutex::boxed(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(mutex.lock());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    // --- events LocalSemaphore ---

    let op = allocs.operation("sync_async_poll_ready/events/LocalSemaphore");
    group.bench_function("events/LocalSemaphore", |b| {
        let sem = LocalSemaphore::boxed(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(sem.acquire());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    // --- Tokio Mutex ---

    let op = allocs.operation("sync_async_poll_ready/tokio/Mutex");
    group.bench_function("tokio/Mutex", |b| {
        let mutex = tokio::sync::Mutex::new(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(mutex.lock());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    // --- Tokio Semaphore ---

    let op = allocs.operation("sync_async_poll_ready/tokio/Semaphore");
    group.bench_function("tokio/Semaphore", |b| {
        let sem = tokio::sync::Semaphore::new(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(sem.acquire());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    // --- async-lock Mutex ---

    let op = allocs.operation("sync_async_poll_ready/async-lock/Mutex");
    group.bench_function("async-lock/Mutex", |b| {
        let mutex = async_lock::Mutex::new(0_u32);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(mutex.lock());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    // --- async-lock Semaphore ---

    let op = allocs.operation("sync_async_poll_ready/async-lock/Semaphore");
    group.bench_function("async-lock/Semaphore", |b| {
        let sem = async_lock::Semaphore::new(1);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(sem.acquire());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    group.finish();
}

// -----------------------------------------------------------------------
// Many waiters
// -----------------------------------------------------------------------

/// Registers `MANY_WAITER_COUNT` waiters on a locked mutex or
/// zero-permit semaphore, then releases them one at a time.
///
/// We use embedded zero-alloc containers to minimize allocation noise.
/// Futures are stored directly in a pre-allocated `Vec` and pinned
/// in-place via `Pin::new_unchecked` (safe because we move them into
/// the `Vec` before any polling and never move them again — `clear()`
/// drops them in place).
fn many_waiters(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("sync_many_waiters");

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);

    // --- events Mutex ---

    let op = allocs.operation("sync_many_waiters/events/Mutex");
    group.bench_function("events/Mutex", |b| {
        let container = pin!(EmbeddedMutex::new(0_u32));
        let mutex = unsafe { Mutex::embedded(container.as_ref()) };

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                // Hold the lock so all waiters block.
                let guard = mutex.try_lock().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| mutex.lock()).take(MANY_WAITER_COUNT));

                // Poll all futures to register them as waiters.
                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                // Release the lock — each guard drop wakes the
                // next waiter in FIFO order.
                drop(guard);

                // Poll each future to complete — each completes
                // and the resulting guard is dropped, waking the
                // next.
                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- events Semaphore ---

    let op = allocs.operation("sync_many_waiters/events/Semaphore");
    group.bench_function("events/Semaphore", |b| {
        let container = pin!(EmbeddedSemaphore::new(1));
        let sem = unsafe { Semaphore::embedded(container.as_ref()) };

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                // Hold the only permit so all waiters block.
                let permit = sem.try_acquire().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| sem.acquire()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                // Release the permit.
                drop(permit);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- events LocalMutex ---

    let op = allocs.operation("sync_many_waiters/events/LocalMutex");
    group.bench_function("events/LocalMutex", |b| {
        let container = pin!(EmbeddedLocalMutex::new(0_u32));
        let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                let guard = mutex.try_lock().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| mutex.lock()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                drop(guard);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- events LocalSemaphore ---

    let op = allocs.operation("sync_many_waiters/events/LocalSemaphore");
    group.bench_function("events/LocalSemaphore", |b| {
        let container = pin!(EmbeddedLocalSemaphore::new(1));
        let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                let permit = sem.try_acquire().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| sem.acquire()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                drop(permit);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- Tokio Mutex ---

    let op = allocs.operation("sync_many_waiters/tokio/Mutex");
    group.bench_function("tokio/Mutex", |b| {
        let mutex = tokio::sync::Mutex::new(0_u32);

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                let guard = mutex.try_lock().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| mutex.lock()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                drop(guard);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- Tokio Semaphore ---

    let op = allocs.operation("sync_many_waiters/tokio/Semaphore");
    group.bench_function("tokio/Semaphore", |b| {
        let sem = tokio::sync::Semaphore::new(1);

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                let permit = sem.try_acquire().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| sem.acquire()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                drop(permit);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- async-lock Mutex ---

    let op = allocs.operation("sync_many_waiters/async-lock/Mutex");
    group.bench_function("async-lock/Mutex", |b| {
        let mutex = async_lock::Mutex::new(0_u32);

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                let guard = mutex.try_lock().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| mutex.lock()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                drop(guard);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    // --- async-lock Semaphore ---

    let op = allocs.operation("sync_many_waiters/async-lock/Semaphore");
    group.bench_function("async-lock/Semaphore", |b| {
        let sem = async_lock::Semaphore::new(1);

        let mut futures: Vec<_> = Vec::with_capacity(MANY_WAITER_COUNT);

        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();

            for _ in 0..iters {
                let permit = sem.try_acquire().unwrap();

                futures.clear();
                futures.extend(iter::repeat_with(|| sem.acquire()).take(MANY_WAITER_COUNT));

                for f in &mut futures {
                    let _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }

                drop(permit);

                for f in &mut futures {
                    let _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }

                futures.clear();
            }

            start.elapsed()
        });
    });

    group.finish();
}
