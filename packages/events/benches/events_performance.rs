//! Benchmarks comparing events package performance to pure oneshot channels.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint;
use std::rc::Rc;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

/// Compares the performance of events package vs pure oneshot channels.
fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("events_performance");

    // Baseline benchmark using pure oneshot channels
    group.bench_function("baseline_oneshot", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            sender.send(hint::black_box(42)).unwrap();
            let value = block_on(async { receiver.recv().unwrap() });
            hint::black_box(value);
        });
    });

    // Thread-safe OnceEvent benchmarks
    group.bench_function("once_event_bind_by_ref", |b| {
        b.iter(|| {
            let event = events::OnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("once_event_bind_by_arc", |b| {
        b.iter(|| {
            let event = Arc::new(events::OnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_arc();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("once_event_bind_by_ptr", |b| {
        b.iter(|| {
            let event = Box::pin(events::OnceEvent::<i32>::new());
            // SAFETY: Event outlives sender and receiver within this closure
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    // Single-threaded LocalOnceEvent benchmarks
    group.bench_function("local_once_event_bind_by_ref", |b| {
        b.iter(|| {
            let event = events::LocalOnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("local_once_event_bind_by_rc", |b| {
        b.iter(|| {
            let event = Rc::new(events::LocalOnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_rc();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("local_once_event_bind_by_ptr", |b| {
        b.iter(|| {
            let event = Box::pin(events::LocalOnceEvent::<i32>::new());
            // SAFETY: Event outlives sender and receiver within this closure
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    // Thread-safe OnceEventPool benchmarks
    group.bench_function("once_event_pool_bind_by_ref", |b| {
        let pool = events::OnceEventPool::<i32>::new();
        b.iter(|| {
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("once_event_pool_bind_by_arc", |b| {
        let pool = Arc::new(events::OnceEventPool::<i32>::new());
        b.iter(|| {
            let (sender, receiver) = pool.bind_by_arc();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("once_event_pool_bind_by_ptr", |b| {
        let pool = Box::pin(events::OnceEventPool::<i32>::new());
        b.iter(|| {
            // SAFETY: Pool outlives sender and receiver within this closure
            let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    // Single-threaded LocalOnceEventPool benchmarks
    group.bench_function("local_once_event_pool_bind_by_ref", |b| {
        let pool = events::LocalOnceEventPool::<i32>::new();
        b.iter(|| {
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("local_once_event_pool_bind_by_rc", |b| {
        let pool = Rc::new(events::LocalOnceEventPool::<i32>::new());
        b.iter(|| {
            let (sender, receiver) = pool.bind_by_rc();
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("local_once_event_pool_bind_by_ptr", |b| {
        let pool = Box::pin(events::LocalOnceEventPool::<i32>::new());
        b.iter(|| {
            // SAFETY: Pool outlives sender and receiver within this closure
            let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
            sender.send(hint::black_box(42));
            let value = block_on(receiver).unwrap();
            hint::black_box(value);
        });
    });

    group.finish();
}
