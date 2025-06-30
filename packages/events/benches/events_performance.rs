//! Benchmarks comparing events package performance to pure oneshot channels.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint;

use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

/// Compares the performance of events package vs pure oneshot channels.
fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("events_vs_oneshot");

    group.bench_function("pure_oneshot_single_thread", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            sender.send(hint::black_box(42)).unwrap();
            let value = block_on(async { receiver.recv().unwrap() });
            hint::black_box(value);
        });
    });

    group.bench_function("events_once_single_thread", |b| {
        b.iter(|| {
            let event = events::once::Event::<i32>::new();
            let (sender, receiver) = event.by_ref();
            sender.send(hint::black_box(42));
            let value = block_on(receiver);
            hint::black_box(value);
        });
    });

    group.finish();
}
