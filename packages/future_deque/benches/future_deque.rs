//! Criterion benchmarks for `FutureDeque` and `LocalFutureDeque`.
//!
//! Each variant has the same five benchmarks: three build-and-drain shapes with different
//! ratios of active futures, and a steady-state churn shape at a small and a large
//! long-lived population.
//!
//! The churn scenarios (`*_transient_churn`) guard steady-state allocation and execution
//! costs. They hold a population of futures that never complete while repeatedly pushing,
//! polling and popping a transient future of the same layout. The allocation report confirms
//! that the bounded workload stops requesting backing storage after warm-up, while the low
//! and high populations expose any occupancy-sensitive execution cost.
//!
//! Allocation counts and processor time are tracked alongside the wall-clock measurement
//! and reported when the benchmark run finishes.

use std::alloc::Layout;
use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use future_deque::{FutureDeque, LocalFutureDeque};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// A future that returns `Pending` for `remaining` polls, then `Ready(value)`.
struct CountdownFuture {
    remaining: usize,
    value: u64,
}

impl Unpin for CountdownFuture {}

impl CountdownFuture {
    fn new(remaining: usize, value: u64) -> Self {
        Self { remaining, value }
    }
}

impl Future for CountdownFuture {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        let this = self.get_mut();
        if this.remaining == 0 {
            Poll::Ready(this.value)
        } else {
            this.remaining = this.remaining.wrapping_sub(1);
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// A future that never completes and never wakes itself.
///
/// The churn scenarios use it for the long-lived population: because it never signals
/// activation, it is polled exactly once and then retains its allocation for the whole run.
///
/// It carries a payload so that it occupies real storage; a zero-sized future would pin
/// nothing and the scenario would lose its point. Its fields match [`CountdownFuture`] so
/// the resident and transient allocations exercise the same allocator size class.
struct NeverReadyFuture {
    remaining: usize,
    value: u64,
}

impl NeverReadyFuture {
    fn new(value: u64) -> Self {
        Self {
            remaining: INACTIVE_POLL_COUNT,
            value,
        }
    }
}

impl Future for NeverReadyFuture {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u64> {
        // Touching the fields keeps the future's layout observable to the optimizer.
        _ = black_box((self.remaining, self.value));
        Poll::Pending
    }
}

/// Keeps the low case large enough to exercise deque traversal without dominating setup.
const FEW_ITEMS: usize = 8;

/// Exposes scaling and allocator-capacity behaviour in the high case.
const MANY_ITEMS: usize = 1000;

/// Represents a sparse active population in the high case.
const ACTIVE_RATIO_LOW: usize = 10;

/// Represents a dense active population in the high case.
const ACTIVE_RATIO_HIGH: usize = 900;

/// Payload of the transient future in the churn scenarios. Only its determinism matters.
const CHURN_VALUE: u64 = 42;

/// Keeps inactive futures pending throughout each benchmark iteration.
const INACTIVE_POLL_COUNT: usize = 1000;

/// Poll budget that makes the transient future complete the first time it is polled.
const READY_ON_FIRST_POLL: usize = 0;

/// Guards the allocator-size-class assumption underpinning the churn scenarios.
fn assert_churn_layouts_match() {
    assert_eq!(
        Layout::new::<NeverReadyFuture>(),
        Layout::new::<CountdownFuture>()
    );
}

/// Builds the steady state shared by the churn scenarios: `long_lived` futures that never
/// complete, polled once so all of them are resident and registered.
///
/// The returned deque is built once per Criterion sample, outside the measured span, and
/// reused across every iteration of that sample, so the long-lived allocations are never
/// attributed to the churn measurement and pin their backing storage for the whole sample.
fn local_deque_with_long_lived(long_lived: usize) -> LocalFutureDeque<u64> {
    assert_churn_layouts_match();

    let mut deque = LocalFutureDeque::new();

    for i in 0..long_lived {
        deque.push_back(NeverReadyFuture::new(i as u64));
    }

    let waker = Waker::noop();
    let cx = &mut Context::from_waker(waker);
    assert!(deque.poll(cx).is_pending());

    // Run one churn cycle here to establish that the measured loop does what the scenario
    // assumes: the transient future completes and is popped, leaving the population intact.
    deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, CHURN_VALUE));
    assert_eq!(deque.poll_back(cx), Poll::Ready(Some(CHURN_VALUE)));
    assert_eq!(deque.len(), long_lived);

    deque
}

/// The [`FutureDeque`] counterpart of [`local_deque_with_long_lived`].
fn sync_deque_with_long_lived(long_lived: usize) -> FutureDeque<u64> {
    assert_churn_layouts_match();

    let mut deque = FutureDeque::new();

    for i in 0..long_lived {
        deque.push_back(NeverReadyFuture::new(i as u64));
    }

    let waker = Waker::noop();
    let cx = &mut Context::from_waker(waker);
    assert!(deque.poll(cx).is_pending());

    deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, CHURN_VALUE));
    assert_eq!(deque.poll_back(cx), Poll::Ready(Some(CHURN_VALUE)));
    assert_eq!(deque.len(), long_lived);

    deque
}

fn bench_local_future_deque(c: &mut Criterion, allocs: &AllocSession, times: &TimeSession) {
    let mut group = c.benchmark_group("future_deque/local");

    let few_items_all_active_alloc = allocs.operation("future_deque/local/few_items_all_active");
    let few_items_all_active_time = times.operation("future_deque/local/few_items_all_active");

    group.bench_function("few_items_all_active", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = few_items_all_active_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = few_items_all_active_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let mut deque = LocalFutureDeque::new();
                for i in 0..FEW_ITEMS {
                    deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, i as u64));
                }
                let waker = Waker::noop();
                let cx = &mut Context::from_waker(waker);
                for _ in 0..FEW_ITEMS {
                    let result = deque.poll_front(cx);
                    let _result = black_box(result);
                }
            }

            start.elapsed()
        });
    });

    let many_items_mostly_inactive_alloc =
        allocs.operation("future_deque/local/many_items_mostly_inactive");
    let many_items_mostly_inactive_time =
        times.operation("future_deque/local/many_items_mostly_inactive");

    group.bench_function("many_items_mostly_inactive", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = many_items_mostly_inactive_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = many_items_mostly_inactive_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let mut deque = LocalFutureDeque::new();
                for i in 0..MANY_ITEMS {
                    let remaining = if i < ACTIVE_RATIO_LOW {
                        READY_ON_FIRST_POLL
                    } else {
                        INACTIVE_POLL_COUNT
                    };
                    deque.push_back(CountdownFuture::new(remaining, i as u64));
                }
                let waker = Waker::noop();
                let cx = &mut Context::from_waker(waker);
                // Poll one round to poll all futures and activate the ones that
                // are immediately ready.
                for _ in 0..ACTIVE_RATIO_LOW {
                    let result = deque.poll_front(cx);
                    let _result = black_box(result);
                }
            }

            start.elapsed()
        });
    });

    let many_items_mostly_active_alloc =
        allocs.operation("future_deque/local/many_items_mostly_active");
    let many_items_mostly_active_time =
        times.operation("future_deque/local/many_items_mostly_active");

    group.bench_function("many_items_mostly_active", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = many_items_mostly_active_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = many_items_mostly_active_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let mut deque = LocalFutureDeque::new();
                for i in 0..MANY_ITEMS {
                    let remaining = if i < ACTIVE_RATIO_HIGH {
                        READY_ON_FIRST_POLL
                    } else {
                        INACTIVE_POLL_COUNT
                    };
                    deque.push_back(CountdownFuture::new(remaining, i as u64));
                }
                let waker = Waker::noop();
                let cx = &mut Context::from_waker(waker);
                for _ in 0..ACTIVE_RATIO_HIGH {
                    let result = deque.poll_front(cx);
                    let _result = black_box(result);
                }
            }

            start.elapsed()
        });
    });

    let few_items_transient_churn_alloc =
        allocs.operation("future_deque/local/few_items_transient_churn");
    let few_items_transient_churn_time =
        times.operation("future_deque/local/few_items_transient_churn");

    group.bench_function("few_items_transient_churn", |b| {
        let mut deque = local_deque_with_long_lived(FEW_ITEMS);

        b.iter_custom(|iterations| {
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);

            let _alloc_span = few_items_transient_churn_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = few_items_transient_churn_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, CHURN_VALUE));
                let result = deque.poll_back(cx);
                let _result = black_box(result);
            }

            start.elapsed()
        });
    });

    let many_items_transient_churn_alloc =
        allocs.operation("future_deque/local/many_items_transient_churn");
    let many_items_transient_churn_time =
        times.operation("future_deque/local/many_items_transient_churn");

    group.bench_function("many_items_transient_churn", |b| {
        let mut deque = local_deque_with_long_lived(MANY_ITEMS);

        b.iter_custom(|iterations| {
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);

            let _alloc_span = many_items_transient_churn_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = many_items_transient_churn_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, CHURN_VALUE));
                let result = deque.poll_back(cx);
                let _result = black_box(result);
            }

            start.elapsed()
        });
    });

    group.finish();
}

fn bench_future_deque(c: &mut Criterion, allocs: &AllocSession, times: &TimeSession) {
    let mut group = c.benchmark_group("future_deque/sync");

    let few_items_all_active_alloc = allocs.operation("future_deque/sync/few_items_all_active");
    let few_items_all_active_time = times.operation("future_deque/sync/few_items_all_active");

    group.bench_function("few_items_all_active", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = few_items_all_active_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = few_items_all_active_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let mut deque = FutureDeque::new();
                for i in 0..FEW_ITEMS {
                    deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, i as u64));
                }
                let waker = Waker::noop();
                let cx = &mut Context::from_waker(waker);
                for _ in 0..FEW_ITEMS {
                    let result = deque.poll_front(cx);
                    let _result = black_box(result);
                }
            }

            start.elapsed()
        });
    });

    let many_items_mostly_inactive_alloc =
        allocs.operation("future_deque/sync/many_items_mostly_inactive");
    let many_items_mostly_inactive_time =
        times.operation("future_deque/sync/many_items_mostly_inactive");

    group.bench_function("many_items_mostly_inactive", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = many_items_mostly_inactive_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = many_items_mostly_inactive_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let mut deque = FutureDeque::new();
                for i in 0..MANY_ITEMS {
                    let remaining = if i < ACTIVE_RATIO_LOW {
                        READY_ON_FIRST_POLL
                    } else {
                        INACTIVE_POLL_COUNT
                    };
                    deque.push_back(CountdownFuture::new(remaining, i as u64));
                }
                let waker = Waker::noop();
                let cx = &mut Context::from_waker(waker);
                for _ in 0..ACTIVE_RATIO_LOW {
                    let result = deque.poll_front(cx);
                    let _result = black_box(result);
                }
            }

            start.elapsed()
        });
    });

    let many_items_mostly_active_alloc =
        allocs.operation("future_deque/sync/many_items_mostly_active");
    let many_items_mostly_active_time =
        times.operation("future_deque/sync/many_items_mostly_active");

    group.bench_function("many_items_mostly_active", |b| {
        b.iter_custom(|iterations| {
            let _alloc_span = many_items_mostly_active_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = many_items_mostly_active_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                let mut deque = FutureDeque::new();
                for i in 0..MANY_ITEMS {
                    let remaining = if i < ACTIVE_RATIO_HIGH {
                        READY_ON_FIRST_POLL
                    } else {
                        INACTIVE_POLL_COUNT
                    };
                    deque.push_back(CountdownFuture::new(remaining, i as u64));
                }
                let waker = Waker::noop();
                let cx = &mut Context::from_waker(waker);
                for _ in 0..ACTIVE_RATIO_HIGH {
                    let result = deque.poll_front(cx);
                    let _result = black_box(result);
                }
            }

            start.elapsed()
        });
    });

    let few_items_transient_churn_alloc =
        allocs.operation("future_deque/sync/few_items_transient_churn");
    let few_items_transient_churn_time =
        times.operation("future_deque/sync/few_items_transient_churn");

    group.bench_function("few_items_transient_churn", |b| {
        let mut deque = sync_deque_with_long_lived(FEW_ITEMS);

        b.iter_custom(|iterations| {
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);

            let _alloc_span = few_items_transient_churn_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = few_items_transient_churn_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, CHURN_VALUE));
                let result = deque.poll_back(cx);
                let _result = black_box(result);
            }

            start.elapsed()
        });
    });

    let many_items_transient_churn_alloc =
        allocs.operation("future_deque/sync/many_items_transient_churn");
    let many_items_transient_churn_time =
        times.operation("future_deque/sync/many_items_transient_churn");

    group.bench_function("many_items_transient_churn", |b| {
        let mut deque = sync_deque_with_long_lived(MANY_ITEMS);

        b.iter_custom(|iterations| {
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);

            let _alloc_span = many_items_transient_churn_alloc
                .measure_thread()
                .iterations(iterations);
            let _time_span = many_items_transient_churn_time
                .measure_thread()
                .iterations(iterations);
            let start = Instant::now();

            for _ in 0..iterations {
                deque.push_back(CountdownFuture::new(READY_ON_FIRST_POLL, CHURN_VALUE));
                let result = deque.poll_back(cx);
                let _result = black_box(result);
            }

            start.elapsed()
        });
    });

    group.finish();
}

fn entrypoint(c: &mut Criterion) {
    let allocs = AllocSession::new();
    let times = TimeSession::new();

    bench_local_future_deque(c, &allocs, &times);
    bench_future_deque(c, &allocs, &times);

    // `allocs` and `times` print their summaries and write JSON to the Cargo
    // target directory when they are dropped at the end of this function.
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
