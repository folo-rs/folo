//! Benchmarks comparing events package performance to pure oneshot channels.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint;

use criterion::{Criterion, criterion_group, criterion_main};

/// Compares the performance of events package vs pure oneshot channels.
fn events_vs_oneshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("events_vs_oneshot");

    group.bench_function("pure_oneshot", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            sender.send(hint::black_box(42)).unwrap();
            let value = receiver.recv().unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("events_once", |b| {
        b.iter(|| {
            let event = events::once::Event::<i32>::new();
            let (sender, receiver) = event.endpoints();
            sender.send(hint::black_box(42));
            let value = receiver.receive();
            hint::black_box(value);
        });
    });

    group.finish();
}

criterion_group!(benches, events_vs_oneshot);
criterion_main!(benches);
