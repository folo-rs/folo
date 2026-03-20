//! Benchmarks comparing `events` with `rsevents` and `event_listener`.
//!
//! Four benchmark groups measure different aspects of event overhead:
//!
//! * **creation** — how expensive is constructing an event object.
//! * **`signal_round_trip`** — non-blocking set + acquire (sync fast path).
//! * **`async_poll_ready`** — create a wait future, pin it, and poll it to
//!   completion on a pre-set event (async fast path).
//! * **`embedded_vs_boxed`** — compares embedded (zero-alloc) and boxed
//!   (heap-allocated) variants side by side.
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
use std::pin::{Pin, pin};
use std::task::{Context, Waker};
use std::time::Instant;

use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use event_listener::Event as ElEvent;
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

    creation(c, &allocs);
    signal_round_trip(c, &allocs);
    async_poll_ready(c, &allocs);
    embedded_vs_boxed(c, &allocs);

    allocs.print_to_stdout();
}

/// Measures the overhead of creating an event object.
fn creation(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("creation");

    let op = allocs.operation("creation/events/ManualResetEvent");
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

    let op = allocs.operation("creation/events/AutoResetEvent");
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

    let op = allocs.operation("creation/events/LocalManualResetEvent");
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

    let op = allocs.operation("creation/events/LocalAutoResetEvent");
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

    let op = allocs.operation("creation/rsevents/ManualResetEvent");
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

    let op = allocs.operation("creation/rsevents/AutoResetEvent");
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

    let op = allocs.operation("creation/event_listener/Event");
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
                black_box(manual.try_acquire());
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
                black_box(local_manual.try_acquire());
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
                black_box(auto.try_acquire());
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
                black_box(local_auto.try_acquire());
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
                let _ = black_box(future.as_mut().poll(&mut cx));
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
                let _ = black_box(future.as_mut().poll(&mut cx));
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
                let _ = black_box(future.as_mut().poll(&mut cx));
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
                let _ = black_box(future.as_mut().poll(&mut cx));
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
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Compares embedded (zero-alloc) and boxed (heap-allocated) event variants.
///
/// Embedded events store their state inline in a pinned container, avoiding
/// any heap allocation for the event itself. This group benchmarks creation,
/// signal round-trip, and async poll for both variants side by side.
fn embedded_vs_boxed(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("embedded_vs_boxed");
    let waker = Waker::noop();

    // --- creation ---

    let op = allocs.operation("embedded_vs_boxed/boxed_ManualResetEvent");
    group.bench_function("boxed/ManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(ManualResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_ManualResetEvent");
    group.bench_function("embedded/ManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = EmbeddedManualResetEvent::new();
                let pinned = unsafe { Pin::new_unchecked(&container) };
                let handle = unsafe { ManualResetEvent::embedded(pinned) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/boxed_AutoResetEvent");
    group.bench_function("boxed/AutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(AutoResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_AutoResetEvent");
    group.bench_function("embedded/AutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = EmbeddedAutoResetEvent::new();
                let pinned = unsafe { Pin::new_unchecked(&container) };
                let handle = unsafe { AutoResetEvent::embedded(pinned) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/boxed_LocalManualResetEvent");
    group.bench_function("boxed/LocalManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(LocalManualResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_LocalManualResetEvent");
    group.bench_function("embedded/LocalManualResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = EmbeddedLocalManualResetEvent::new();
                let pinned = unsafe { Pin::new_unchecked(&container) };
                let handle = unsafe { LocalManualResetEvent::embedded(pinned) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/boxed_LocalAutoResetEvent");
    group.bench_function("boxed/LocalAutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(LocalAutoResetEvent::boxed());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_LocalAutoResetEvent");
    group.bench_function("embedded/LocalAutoResetEvent", |b| {
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let container = EmbeddedLocalAutoResetEvent::new();
                let pinned = unsafe { Pin::new_unchecked(&container) };
                let handle = unsafe { LocalAutoResetEvent::embedded(pinned) };
                black_box(handle);
            }
            start.elapsed()
        });
    });

    // --- signal round-trip (embedded) ---

    let op = allocs.operation("embedded_vs_boxed/embedded_ManualResetEvent_signal");
    group.bench_function("embedded/ManualResetEvent/signal", |b| {
        let manual_container = Box::pin(EmbeddedManualResetEvent::new());
        let manual = unsafe { ManualResetEvent::embedded(manual_container.as_ref()) };
        manual.set();
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                black_box(manual.try_acquire());
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_AutoResetEvent_signal");
    group.bench_function("embedded/AutoResetEvent/signal", |b| {
        let auto_container = Box::pin(EmbeddedAutoResetEvent::new());
        let auto = unsafe { AutoResetEvent::embedded(auto_container.as_ref()) };
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                auto.set();
                black_box(auto.try_acquire());
            }
            start.elapsed()
        });
    });

    // --- async poll (embedded) ---

    let op = allocs.operation("embedded_vs_boxed/embedded_ManualResetEvent_poll");
    group.bench_function("embedded/ManualResetEvent/poll", |b| {
        let manual_container = Box::pin(EmbeddedManualResetEvent::new());
        let manual = unsafe { ManualResetEvent::embedded(manual_container.as_ref()) };
        manual.set();
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                let mut future = pin!(manual.wait());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_AutoResetEvent_poll");
    group.bench_function("embedded/AutoResetEvent/poll", |b| {
        let auto_container = Box::pin(EmbeddedAutoResetEvent::new());
        let auto = unsafe { AutoResetEvent::embedded(auto_container.as_ref()) };
        let mut cx = Context::from_waker(waker);
        b.iter_custom(|iters| {
            let _span = op.measure_thread().iterations(iters);
            let start = Instant::now();
            for _ in 0..iters {
                auto.set();
                let mut future = pin!(auto.wait());
                let _ = black_box(future.as_mut().poll(&mut cx));
            }
            start.elapsed()
        });
    });

    group.finish();
}
