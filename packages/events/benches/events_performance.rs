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

    group.bench_function("pure_oneshot_single_thread", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            sender.send(hint::black_box(42)).unwrap();
            let value = receiver.recv().unwrap();
            hint::black_box(value);
        });
    });

    group.bench_function("events_once_single_thread", |b| {
        b.iter(|| {
            let event = events::once::Event::<i32>::new();
            let (sender, receiver) = event.by_ref();
            sender.send(hint::black_box(42));
            let value = receiver.recv();
            hint::black_box(value);
        });
    });

    group.bench_function("local_events_once_single_thread", |b| {
        b.iter(|| {
            let event = events::once::LocalEvent::<i32>::new();
            let (sender, receiver) = event.by_ref();
            sender.send(hint::black_box(42));
            let value = receiver.recv();
            hint::black_box(value);
        });
    });

    group.finish();
}

/// Benchmarks creation overhead for different event types.
fn event_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_creation");

    group.bench_function("pure_oneshot_creation", |b| {
        b.iter(|| {
            let (sender, receiver) = oneshot::channel::<i32>();
            hint::black_box((sender, receiver));
        });
    });

    group.bench_function("thread_safe_event_creation", |b| {
        b.iter(|| {
            let event = events::once::Event::<i32>::new();
            hint::black_box(event);
        });
    });

    group.bench_function("local_event_creation", |b| {
        b.iter(|| {
            let event = events::once::LocalEvent::<i32>::new();
            hint::black_box(event);
        });
    });

    group.bench_function("thread_safe_event_with_endpoints", |b| {
        b.iter(|| {
            let event = events::once::Event::<i32>::new();
            let endpoints = event.by_ref();
            hint::black_box(endpoints);
        });
    });

    group.bench_function("local_event_with_endpoints", |b| {
        b.iter(|| {
            let event = events::once::LocalEvent::<i32>::new();
            let endpoints = event.by_ref();
            hint::black_box(endpoints);
        });
    });

    group.finish();
}

criterion_group!(benches, events_vs_oneshot, event_creation);
criterion_main!(benches);
