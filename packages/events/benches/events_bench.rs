//! Benchmarks comparing `events` with `rsevents` and `event_listener`.
//!
//! Five benchmark groups measure different aspects of event overhead:
//!
//! * **`creation_boxed`** — how expensive is constructing a heap-allocated
//!   event object.
//! * **`creation_embedded`** — how expensive is constructing a zero-alloc
//!   embedded event object (includes competitor crates that also avoid
//!   heap allocation).
//! * **`signal_round_trip`** — non-blocking set + acquire (sync fast path).
//! * **`async_poll_ready`** — create a wait future, pin it, and poll it to
//!   completion on a pre-set event (async fast path).
//! * **`many_waiters`** — register 100 waiters and release them in one batch.
//!
//! Memory allocations are tracked via `alloc_tracker` and printed to stdout
//! after all benchmarks complete.

#![allow(missing_docs, reason = "benchmark code")]
#![allow(
    clippy::let_underscore_must_use,
    reason = "poll results are intentionally discarded in benchmarks"
)]
#![allow(
    clippy::undocumented_unsafe_blocks,
    reason = "benchmark pinning has trivial safety invariants"
)]

use std::future::Future;
use std::hint::black_box;
use std::iter;
use std::pin::{Pin, pin};
use std::task::{Context, Waker};
use std::time::Instant;

use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use event_listener::{Event as ElEvent, listener};
use events::{
    AutoResetEvent, EmbeddedAutoResetEvent, EmbeddedLocalAutoResetEvent,
    EmbeddedLocalManualResetEvent, EmbeddedManualResetEvent, LocalAutoResetEvent,
    LocalManualResetEvent, ManualResetEvent,
};
use rsevents::{
    AutoResetEvent as RsAutoReset, Awaitable as _, EventState, ManualResetEvent as RsManualReset,
};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let allocs = AllocSession::new();

    creation_boxed(c, &allocs);
    creation_embedded(c, &allocs);
    signal_round_trip(c, &allocs);
    async_poll_ready(c, &allocs);
    many_waiters(c, &allocs);

    allocs.print_to_stdout();
}

/// Measures the overhead of creating a heap-allocated event object.
fn creation_boxed(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("creation_boxed");

    let op = allocs.operation("creation_boxed/events/ManualResetEvent");
    group.bench_function("events/ManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(ManualResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_boxed/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(AutoResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_boxed/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(LocalManualResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_boxed/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(LocalAutoResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Measures the overhead of creating an embedded (zero-alloc) event object.
fn creation_embedded(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("creation_embedded");

    let op = allocs.operation("creation_embedded/events/ManualResetEvent");
    group.bench_function("events/embedded/ManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedManualResetEvent::new());
                let handle = unsafe { ManualResetEvent::embedded(container.as_ref()) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_embedded/events/AutoResetEvent");
    group.bench_function("events/embedded/AutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedAutoResetEvent::new());
                let handle = unsafe { AutoResetEvent::embedded(container.as_ref()) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_embedded/events/LocalManualResetEvent");
    group.bench_function("events/embedded/LocalManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedLocalManualResetEvent::new());
                let handle = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_embedded/events/LocalAutoResetEvent");
    group.bench_function("events/embedded/LocalAutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedLocalAutoResetEvent::new());
                let handle = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_embedded/rsevents/ManualResetEvent");
    group.bench_function("rsevents/ManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(RsManualReset::new(EventState::Unset));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_embedded/rsevents/AutoResetEvent");
    group.bench_function("rsevents/AutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(RsAutoReset::new(EventState::Unset));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("creation_embedded/event_listener/Event");
    group.bench_function("event_listener/Event", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(ElEvent::<()>::new());
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Measures the non-blocking signal round-trip: set the event and then
/// synchronously consume or verify the signal.
///
/// Manual-reset events stay set, so each iteration is a non-consuming check.
/// Auto-reset events consume the signal, so each iteration re-sets the event.
/// The `rsevents` equivalents use blocking `wait()` on pre-set events, which
/// returns immediately on the fast path without parking the thread.
fn signal_round_trip(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("signal_round_trip");

    let op = allocs.operation("signal_round_trip/events/ManualResetEvent");
    group.bench_function("events/ManualResetEvent", |b| {
        let manual = ManualResetEvent::boxed();
        manual.set();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(manual.try_wait());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        let local_manual = LocalManualResetEvent::boxed();
        local_manual.set();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(local_manual.try_wait());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        let auto = AutoResetEvent::boxed();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                auto.set();
                black_box(auto.try_wait());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        let local_auto = LocalAutoResetEvent::boxed();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                local_auto.set();
                black_box(local_auto.try_wait());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/events/embedded/ManualResetEvent");
    group.bench_function("events/embedded/ManualResetEvent", |b| {
        let container = pin!(EmbeddedManualResetEvent::new());
        let manual = unsafe { ManualResetEvent::embedded(container.as_ref()) };
        manual.set();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(manual.try_wait());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/events/embedded/AutoResetEvent");
    group.bench_function("events/embedded/AutoResetEvent", |b| {
        let container = pin!(EmbeddedAutoResetEvent::new());
        let auto = unsafe { AutoResetEvent::embedded(container.as_ref()) };
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                auto.set();
                black_box(auto.try_wait());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/event_listener/Event");
    group.bench_function("event_listener/Event", |b| {
        let el_event = ElEvent::<()>::new();
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                el_event.notify(1);
                let mut future = pin!(el_event.listen());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/event_listener/Event (listener!)");
    group.bench_function("event_listener/Event (listener!)", |b| {
        let el_event = ElEvent::<()>::new();
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                el_event.notify(1);
                listener!(el_event => el_listener);
                let mut el_listener = pin!(el_listener);
                _ = black_box(Future::poll(el_listener.as_mut(), &mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/rsevents/ManualResetEvent");
    group.bench_function("rsevents/ManualResetEvent", |b| {
        let rs_manual = RsManualReset::new(EventState::Set);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                rs_manual.wait();
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("signal_round_trip/rsevents/AutoResetEvent");
    group.bench_function("rsevents/AutoResetEvent", |b| {
        let rs_auto = RsAutoReset::new(EventState::Unset);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                rs_auto.set();
                rs_auto.wait();
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Measures the async fast path: set the event, create a wait future, pin it,
/// and poll it to completion in a single poll.
///
/// Futures are pinned on the stack via the `pin!()` macro, avoiding heap
/// allocations from `Box::pin()`.
fn async_poll_ready(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("async_poll_ready");
    let waker = Waker::noop();

    let op = allocs.operation("async_poll_ready/events/ManualResetEvent");
    group.bench_function("events/ManualResetEvent", |b| {
        let manual = ManualResetEvent::boxed();
        manual.set();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(manual.wait());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        let local_manual = LocalManualResetEvent::boxed();
        local_manual.set();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(local_manual.wait());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        let auto = AutoResetEvent::boxed();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                auto.set();
                let mut future = pin!(auto.wait());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        let local_auto = LocalAutoResetEvent::boxed();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                local_auto.set();
                let mut future = pin!(local_auto.wait());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/events/embedded/ManualResetEvent");
    group.bench_function("events/embedded/ManualResetEvent", |b| {
        let container = pin!(EmbeddedManualResetEvent::new());
        let manual = unsafe { ManualResetEvent::embedded(container.as_ref()) };
        manual.set();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(manual.wait());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/events/embedded/AutoResetEvent");
    group.bench_function("events/embedded/AutoResetEvent", |b| {
        let container = pin!(EmbeddedAutoResetEvent::new());
        let auto = unsafe { AutoResetEvent::embedded(container.as_ref()) };
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                auto.set();
                let mut future = pin!(auto.wait());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/event_listener/Event");
    group.bench_function("event_listener/Event", |b| {
        let el_event = ElEvent::<()>::new();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                el_event.notify(1);
                let mut future = pin!(el_event.listen());
                _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("async_poll_ready/event_listener/Event (listener!)");
    group.bench_function("event_listener/Event (listener!)", |b| {
        let el_event = ElEvent::<()>::new();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                el_event.notify(1);
                listener!(el_event => el_listener);
                let mut el_listener = pin!(el_listener);
                _ = black_box(Future::poll(el_listener.as_mut(), &mut cx));
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Number of waiters used in the `many_waiters` benchmark group.
const MANY_WAITER_COUNT: usize = 100;

/// Measures the cost of releasing many waiters at once.
///
/// For manual-reset events, a single `set()` releases all waiters. For
/// auto-reset events, each waiter requires its own `set()` call. All
/// waiters are pre-registered before the measurement begins.
///
/// Events use embedded (zero-alloc) containers to minimize allocation
/// noise. Futures are stored directly in a pre-allocated `Vec` and
/// pinned in-place via `Pin::new_unchecked`. The pinning is sound
/// because:
///
/// * Futures are moved into the `Vec` before any polling (no
///   self-referential state yet).
/// * After polling (which links them into the waiter list), they are
///   never moved — the `Vec` does not reallocate because it has enough
///   capacity from a previous iteration.
/// * `clear()` drops futures in place, unlinking them from the waiter
///   list.
fn many_waiters(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("many_waiters");
    let waker = Waker::noop();

    let op = allocs.operation("many_waiters/events/ManualResetEvent");
    group.bench_function("events/ManualResetEvent", |b| {
        let mut cx = Context::from_waker(waker);
        let mut futures = Vec::new();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedManualResetEvent::new());
                let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
                futures.extend(iter::repeat_with(|| event.wait()).take(MANY_WAITER_COUNT));
                for f in &mut futures {
                    _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }
                event.set();
                for f in &mut futures {
                    _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }
                // Clear before the container goes out of scope.
                futures.clear();
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("many_waiters/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        let mut cx = Context::from_waker(waker);
        let mut futures = Vec::new();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedLocalManualResetEvent::new());
                let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
                futures.extend(iter::repeat_with(|| event.wait()).take(MANY_WAITER_COUNT));
                for f in &mut futures {
                    _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }
                event.set();
                for f in &mut futures {
                    _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }
                futures.clear();
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("many_waiters/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        let mut cx = Context::from_waker(waker);
        let mut futures = Vec::new();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedAutoResetEvent::new());
                let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
                futures.extend(iter::repeat_with(|| event.wait()).take(MANY_WAITER_COUNT));
                for f in &mut futures {
                    _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }
                for f in &mut futures {
                    event.set();
                    _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }
                futures.clear();
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("many_waiters/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        let mut cx = Context::from_waker(waker);
        let mut futures = Vec::new();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = pin!(EmbeddedLocalAutoResetEvent::new());
                let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
                futures.extend(iter::repeat_with(|| event.wait()).take(MANY_WAITER_COUNT));
                for f in &mut futures {
                    _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }
                for f in &mut futures {
                    event.set();
                    _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }
                futures.clear();
            }
            start.elapsed()
        });
    });

    // rsevents uses synchronous blocking and cannot register multiple
    // async waiters, so it is excluded from this benchmark group.

    let op = allocs.operation("many_waiters/event_listener/Event");
    group.bench_function("event_listener/Event", |b| {
        let mut cx = Context::from_waker(waker);
        let mut futures = Vec::new();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let el_event = ElEvent::<()>::new();
                futures.clear();
                futures.extend(iter::repeat_with(|| el_event.listen()).take(MANY_WAITER_COUNT));
                for f in &mut futures {
                    _ = unsafe { Pin::new_unchecked(f) }.poll(&mut cx);
                }
                el_event.notify(MANY_WAITER_COUNT);
                for f in &mut futures {
                    _ = black_box(unsafe { Pin::new_unchecked(f) }.poll(&mut cx));
                }
            }
            start.elapsed()
        });
    });

    group.finish();
}
