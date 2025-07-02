//! Multi-threaded performance benchmarks for events package.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use criterion::{criterion_group, criterion_main, Criterion};
use events::{OnceEvent, OnceEventPool};
use futures::executor::block_on;
use many_cpus_benchmarking::{execute_runs, Payload, WorkDistribution};
use std::sync::Arc;

/// Benchmark payload for testing Arc binding of individual events.
#[derive(Debug, Default)]
struct EventArcBinding;

impl Payload for EventArcBinding {
    fn new_pair() -> (Self, Self) {
        (Self, Self)
    }

    fn process(&mut self) {
        // Create a fresh event for this iteration
        let event = Arc::new(OnceEvent::<u64>::new());
        let (sender, receiver) = event.bind_by_arc();
        
        // Send a value
        sender.send(42);
        
        // Receive the value
        let value = block_on(receiver).expect("Failed to receive value");
        assert_eq!(value, 42);
    }
}

/// Benchmark payload for testing Ptr binding of individual events.
#[derive(Debug, Default)]
struct EventPtrBinding;

impl Payload for EventPtrBinding {
    fn new_pair() -> (Self, Self) {
        (Self, Self)
    }

    fn process(&mut self) {
        // Create a fresh event for this iteration
        let event = Box::pin(OnceEvent::<u64>::new());
        // SAFETY: The event is pinned and will remain valid for the duration of the binding.
        let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
        
        // Send a value
        sender.send(42);
        
        // Receive the value
        let value = block_on(receiver).expect("Failed to receive value");
        assert_eq!(value, 42);
    }
}

/// Benchmark payload for testing Arc binding of pooled events.
#[derive(Debug)]
struct EventPoolArcBinding {
    pool: Arc<OnceEventPool<u64>>,
}

impl Payload for EventPoolArcBinding {
    fn new_pair() -> (Self, Self) {
        let pool = Arc::new(OnceEventPool::new());
        (
            Self { pool: Arc::clone(&pool) },
            Self { pool },
        )
    }

    fn process(&mut self) {
        // Get a fresh event from the pool for this iteration
        let (sender, receiver) = self.pool.bind_by_arc();
        
        // Send a value
        sender.send(42);
        
        // Receive the value
        let value = block_on(receiver).expect("Failed to receive value");
        assert_eq!(value, 42);
    }
}

/// Benchmark payload for testing Ptr binding of pooled events.
#[derive(Debug)]
struct EventPoolPtrBinding {
    pool: std::pin::Pin<Box<OnceEventPool<u64>>>,
}

impl Payload for EventPoolPtrBinding {
    fn new_pair() -> (Self, Self) {
        (
            Self { pool: Box::pin(OnceEventPool::new()) },
            Self { pool: Box::pin(OnceEventPool::new()) },
        )
    }

    fn process(&mut self) {
        // Get a fresh event from the pool for this iteration
        // SAFETY: The pool is pinned and will remain valid for the duration of the binding.
        let (sender, receiver) = unsafe { self.pool.as_ref().bind_by_ptr() };
        
        // Send a value
        sender.send(42);
        
        // Receive the value
        let value = block_on(receiver).expect("Failed to receive value");
        assert_eq!(value, 42);
    }
}

/// Baseline benchmark using oneshot channels for comparison.
#[derive(Debug, Default)]
struct BaselineOneshotChannel;

impl Payload for BaselineOneshotChannel {
    fn new_pair() -> (Self, Self) {
        (Self, Self)
    }

    fn process(&mut self) {
        // Create a fresh oneshot channel for this iteration
        let (sender, receiver) = oneshot::channel::<u64>();
        
        // Send a value
        sender.send(42).expect("Failed to send value");
        
        // Receive the value
        let value = block_on(receiver).expect("Failed to receive value");
        assert_eq!(value, 42);
    }
}

fn benchmark_events_multithreaded(c: &mut Criterion) {
    // Run all benchmarks with a reasonable batch size
    execute_runs::<EventArcBinding, 10>(c, WorkDistribution::all_with_unique_processors());
    execute_runs::<EventPtrBinding, 10>(c, WorkDistribution::all_with_unique_processors());
    execute_runs::<EventPoolArcBinding, 10>(c, WorkDistribution::all_with_unique_processors());
    execute_runs::<EventPoolPtrBinding, 10>(c, WorkDistribution::all_with_unique_processors());
    execute_runs::<BaselineOneshotChannel, 10>(c, WorkDistribution::all_with_unique_processors());
}

criterion_group!(benches, benchmark_events_multithreaded);
criterion_main!(benches);
