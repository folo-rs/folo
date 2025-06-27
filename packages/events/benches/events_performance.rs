//! Benchmarks comparing events package performance to pure oneshot channels.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

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

/// Cross-thread communication benchmark using events package.
/// One worker sends a value, the other receives it.
#[derive(Debug)]
struct EventsCrossThread {
    event: Option<Arc<events::once::Event<i32>>>,
    is_sender: bool,
    value: Option<i32>,
}

impl Default for EventsCrossThread {
    fn default() -> Self {
        Self {
            event: None,
            is_sender: false,
            value: None,
        }
    }
}

impl Payload for EventsCrossThread {
    fn new_pair() -> (Self, Self) {
        let event = Arc::new(events::once::Event::<i32>::new());

        (
            Self {
                event: Some(event.clone()),
                is_sender: true,
                value: Some(42),
            },
            Self {
                event: Some(event),
                is_sender: false,
                value: None,
            },
        )
    }

    fn process(&mut self) {
        let event = self.event.take().unwrap();
        if self.is_sender {
            // Sender worker
            let sender = event.sender();
            let value = self.value.take().unwrap();
            sender.send(hint::black_box(value));
        } else {
            // Receiver worker
            let receiver = event.receiver();
            let value = receiver.receive();
            hint::black_box(value);
        }
    }
}

/// Cross-thread communication benchmark using pure oneshot channels.
/// One worker sends a value, the other receives it.
#[derive(Debug, Default)]
struct OneshotCrossThread {
    sender: Option<oneshot::Sender<i32>>,
    receiver: Option<oneshot::Receiver<i32>>,
    value: Option<i32>,
}

impl Payload for OneshotCrossThread {
    fn new_pair() -> (Self, Self) {
        let (sender, receiver) = oneshot::channel();

        (
            Self {
                sender: Some(sender),
                receiver: None,
                value: Some(42),
            },
            Self {
                sender: None,
                receiver: Some(receiver),
                value: None,
            },
        )
    }

    fn process(&mut self) {
        if let Some(sender) = self.sender.take() {
            // Sender worker
            let value = self.value.take().unwrap();
            sender.send(hint::black_box(value)).unwrap();
        } else if let Some(receiver) = self.receiver.take() {
            // Receiver worker
            let value = receiver.recv().unwrap();
            hint::black_box(value);
        }
    }
}

/// Benchmarks cross-thread communication scenarios.
fn cross_thread_events(c: &mut Criterion) {
    execute_runs::<EventsCrossThread, 100>(c, WorkDistribution::all_with_unique_processors());
    execute_runs::<OneshotCrossThread, 100>(c, WorkDistribution::all_with_unique_processors());
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

criterion_group!(
    benches,
    events_vs_oneshot,
    cross_thread_events,
    event_creation
);
criterion_main!(benches);
