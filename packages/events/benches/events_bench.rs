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

use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Wake, Waker};

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

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn noop_waker() -> Waker {
    Waker::from(Arc::new(NoopWake))
}

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
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(ManualResetEvent::boxed())
        });
    });

    let op = allocs.operation("creation/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(AutoResetEvent::boxed())
        });
    });

    let op = allocs.operation("creation/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(LocalManualResetEvent::boxed())
        });
    });

    let op = allocs.operation("creation/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(LocalAutoResetEvent::boxed())
        });
    });

    let op = allocs.operation("creation/rsevents/ManualResetEvent");
    group.bench_function("rsevents/ManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(RsManualReset::new(EventState::Unset))
        });
    });

    let op = allocs.operation("creation/rsevents/AutoResetEvent");
    group.bench_function("rsevents/AutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(RsAutoReset::new(EventState::Unset))
        });
    });

    let op = allocs.operation("creation/event_listener/Event");
    group.bench_function("event_listener/Event", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(ElEvent::<()>::new())
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

    // Manual-reset: pre-set, try_acquire() is a non-consuming check.
    let manual = ManualResetEvent::boxed();
    manual.set();
    let op = allocs.operation("signal_round_trip/events/ManualResetEvent");
    group.bench_function("events/ManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(manual.try_acquire())
        });
    });

    let local_manual = LocalManualResetEvent::boxed();
    local_manual.set();
    let op = allocs.operation("signal_round_trip/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(local_manual.try_acquire())
        });
    });

    // Auto-reset: consumes the signal, so re-set each iteration.
    let auto = AutoResetEvent::boxed();
    let op = allocs.operation("signal_round_trip/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            auto.set();
            black_box(auto.try_acquire())
        });
    });

    let local_auto = LocalAutoResetEvent::boxed();
    let op = allocs.operation("signal_round_trip/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            local_auto.set();
            black_box(local_auto.try_acquire())
        });
    });

    // rsevents: ManualResetEvent stays set — wait() returns immediately.
    let rs_manual = RsManualReset::new(EventState::Set);
    let op = allocs.operation("signal_round_trip/rsevents/ManualResetEvent");
    group.bench_function("rsevents/ManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            rs_manual.wait();
        });
    });

    // rsevents: AutoResetEvent consumes signal on wait() — re-set per
    // iteration.
    let rs_auto = RsAutoReset::new(EventState::Unset);
    let op = allocs.operation("signal_round_trip/rsevents/AutoResetEvent");
    group.bench_function("rsevents/AutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            rs_auto.set();
            rs_auto.wait();
        });
    });

    group.finish();
}

/// Measures the async fast path: set the event, create a wait future, pin it,
/// and poll it to completion in a single poll.
///
/// All implementations use `Box::pin` for pinning, ensuring a level playing
/// field. The `Box` allocation overhead is constant across all implementations,
/// keeping relative comparisons valid.
fn async_poll_ready(c: &mut Criterion, allocs: &AllocSession) {
    let mut group = c.benchmark_group("async_poll_ready");

    // ManualResetEvent — pre-set, poll returns Ready immediately.
    let manual = ManualResetEvent::boxed();
    manual.set();
    let op = allocs.operation("async_poll_ready/events/ManualResetEvent");
    group.bench_function("events/ManualResetEvent", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            let mut future = Box::pin(manual.wait());
            let _ = black_box(future.as_mut().poll(&mut cx));
        });
    });

    let local_manual = LocalManualResetEvent::boxed();
    local_manual.set();
    let op = allocs.operation("async_poll_ready/events/LocalManualResetEvent");
    group.bench_function("events/LocalManualResetEvent", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            let mut future = Box::pin(local_manual.wait());
            let _ = black_box(future.as_mut().poll(&mut cx));
        });
    });

    // AutoResetEvent — re-set each iteration because polling consumes the
    // signal.
    let auto = AutoResetEvent::boxed();
    let op = allocs.operation("async_poll_ready/events/AutoResetEvent");
    group.bench_function("events/AutoResetEvent", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            auto.set();
            let mut future = Box::pin(auto.wait());
            let _ = black_box(future.as_mut().poll(&mut cx));
        });
    });

    let local_auto = LocalAutoResetEvent::boxed();
    let op = allocs.operation("async_poll_ready/events/LocalAutoResetEvent");
    group.bench_function("events/LocalAutoResetEvent", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            local_auto.set();
            let mut future = Box::pin(local_auto.wait());
            let _ = black_box(future.as_mut().poll(&mut cx));
        });
    });

    // event_listener — notify(1) stores signal, listen() + poll picks it up.
    let el_event = ElEvent::<()>::new();
    let op = allocs.operation("async_poll_ready/event_listener/Event");
    group.bench_function("event_listener/Event", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            el_event.notify(1);
            let listener = el_event.listen();
            let mut pinned = Box::pin(listener);
            let _ = black_box(pinned.as_mut().poll(&mut cx));
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

    // --- creation ---

    let op = allocs.operation("embedded_vs_boxed/boxed_ManualResetEvent");
    group.bench_function("boxed/ManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(ManualResetEvent::boxed())
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_ManualResetEvent");
    group.bench_function("embedded/ManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            let container = EmbeddedManualResetEvent::new();
            // SAFETY: container is not moved (stays on the stack) and
            // outlives the handle within this iteration.
            let pinned = unsafe { Pin::new_unchecked(&container) };
            // SAFETY: The container outlives the handle (same scope).
            let handle = unsafe { ManualResetEvent::embedded(pinned) };
            black_box(handle);
        });
    });

    let op = allocs.operation("embedded_vs_boxed/boxed_AutoResetEvent");
    group.bench_function("boxed/AutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(AutoResetEvent::boxed())
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_AutoResetEvent");
    group.bench_function("embedded/AutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            let container = EmbeddedAutoResetEvent::new();
            // SAFETY: container is not moved and outlives the handle.
            let pinned = unsafe { Pin::new_unchecked(&container) };
            // SAFETY: The container outlives the handle (same scope).
            let handle = unsafe { AutoResetEvent::embedded(pinned) };
            black_box(handle);
        });
    });

    let op = allocs.operation("embedded_vs_boxed/boxed_LocalManualResetEvent");
    group.bench_function("boxed/LocalManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(LocalManualResetEvent::boxed())
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_LocalManualResetEvent");
    group.bench_function("embedded/LocalManualResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            let container = EmbeddedLocalManualResetEvent::new();
            // SAFETY: container is not moved and outlives the handle.
            let pinned = unsafe { Pin::new_unchecked(&container) };
            // SAFETY: The container outlives the handle (same scope).
            let handle = unsafe { LocalManualResetEvent::embedded(pinned) };
            black_box(handle);
        });
    });

    let op = allocs.operation("embedded_vs_boxed/boxed_LocalAutoResetEvent");
    group.bench_function("boxed/LocalAutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(LocalAutoResetEvent::boxed())
        });
    });

    let op = allocs.operation("embedded_vs_boxed/embedded_LocalAutoResetEvent");
    group.bench_function("embedded/LocalAutoResetEvent", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            let container = EmbeddedLocalAutoResetEvent::new();
            // SAFETY: container is not moved and outlives the handle.
            let pinned = unsafe { Pin::new_unchecked(&container) };
            // SAFETY: The container outlives the handle (same scope).
            let handle = unsafe { LocalAutoResetEvent::embedded(pinned) };
            black_box(handle);
        });
    });

    // --- signal round-trip (embedded) ---

    let manual_container = Box::pin(EmbeddedManualResetEvent::new());
    // SAFETY: The container outlives the benchmark closure.
    let manual = unsafe { ManualResetEvent::embedded(manual_container.as_ref()) };
    manual.set();
    let op = allocs.operation("embedded_vs_boxed/embedded_ManualResetEvent_signal");
    group.bench_function("embedded/ManualResetEvent/signal", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            black_box(manual.try_acquire())
        });
    });

    let auto_container = Box::pin(EmbeddedAutoResetEvent::new());
    // SAFETY: The container outlives the benchmark closure.
    let auto = unsafe { AutoResetEvent::embedded(auto_container.as_ref()) };
    let op = allocs.operation("embedded_vs_boxed/embedded_AutoResetEvent_signal");
    group.bench_function("embedded/AutoResetEvent/signal", |b| {
        b.iter(|| {
            let _span = op.measure_thread();
            auto.set();
            black_box(auto.try_acquire())
        });
    });

    // --- async poll (embedded) ---

    let manual_container = Box::pin(EmbeddedManualResetEvent::new());
    // SAFETY: The container outlives the benchmark closure.
    let manual = unsafe { ManualResetEvent::embedded(manual_container.as_ref()) };
    manual.set();
    let op = allocs.operation("embedded_vs_boxed/embedded_ManualResetEvent_poll");
    group.bench_function("embedded/ManualResetEvent/poll", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            let mut future = Box::pin(manual.wait());
            let _ = black_box(future.as_mut().poll(&mut cx));
        });
    });

    let auto_container = Box::pin(EmbeddedAutoResetEvent::new());
    // SAFETY: The container outlives the benchmark closure.
    let auto = unsafe { AutoResetEvent::embedded(auto_container.as_ref()) };
    let op = allocs.operation("embedded_vs_boxed/embedded_AutoResetEvent_poll");
    group.bench_function("embedded/AutoResetEvent/poll", |b| {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        b.iter(|| {
            let _span = op.measure_thread();
            auto.set();
            let mut future = Box::pin(auto.wait());
            let _ = black_box(future.as_mut().poll(&mut cx));
        });
    });

    group.finish();
}
