//! Benchmarks comparing events package performance to pure oneshot channels.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint;

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;

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
            let (sender, receiver) = event.endpoints();
            sender.send(hint::black_box(42));
            let value = receiver.receive();
            hint::black_box(value);
        });
    });

    group.bench_function("local_events_once_single_thread", |b| {
        b.iter(|| {
            let event = events::once::LocalEvent::<i32>::new();
            let (sender, receiver) = event.endpoints();
            sender.send(hint::black_box(42));
            let value = receiver.receive();
            hint::black_box(value);
        });
    });

    group.finish();
}

/// Benchmarks cross-thread communication scenarios.
fn cross_thread_events(c: &mut Criterion) {
    let two_threads = ProcessorSet::builder()
        .performance_processors_only()
        .take(nz!(2));
    
    if let Some(processor_set) = two_threads {
        let thread_pool = ThreadPool::new(&processor_set);
        let mut group = c.benchmark_group("cross_thread");

        group.bench_function("events_cross_thread_create_endpoints", |b| {
            b.iter_custom(|iters| {
                bench_on_threadpool(
                    &thread_pool,
                    iters,
                    || (), // No preparation needed
                    |()| {
                        // Each thread creates its own event and endpoints
                        let event = events::once::Event::<i32>::new();
                        let (sender, receiver) = event.endpoints();
                        sender.send(hint::black_box(42));
                        let value = receiver.receive();
                        hint::black_box(value);
                    },
                )
            });
        });

        group.finish();
    }
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
            let endpoints = event.endpoints();
            hint::black_box(endpoints);
        });
    });

    group.bench_function("local_event_with_endpoints", |b| {
        b.iter(|| {
            let event = events::once::LocalEvent::<i32>::new();
            let endpoints = event.endpoints();
            hint::black_box(endpoints);
        });
    });

    group.finish();
}

criterion_group!(benches, events_vs_oneshot, cross_thread_events, event_creation);
criterion_main!(benches);
