//! Metrics for the vicinal worker pool.
//!
//! This module provides events for observing task scheduling and execution latencies.
//! The metrics use per-thread event instances to minimize contention.

use nm::{Event, Magnitude};

/// Histogram buckets for scheduling delay in milliseconds.
///
/// The scheduling delay is the time between when a task is spawned and when it starts executing.
/// We expect most tasks to be picked up within a few milliseconds when the worker is not busy.
const SCHEDULING_DELAY_MS_BUCKETS: &[Magnitude] = &[0, 1, 2, 5, 10, 20, 50, 100, 200, 500, 1000];

/// Histogram buckets for task execution time in milliseconds.
///
/// The execution time is how long the task function takes to run.
/// We expect a wide distribution since tasks can be anything from simple to complex.
const EXECUTION_TIME_MS_BUCKETS: &[Magnitude] = &[0, 1, 5, 10, 25, 50, 100, 250, 500, 1000, 5000];

thread_local! {
    /// Event for observing the delay between task spawn and task execution start.
    ///
    /// The magnitude is the delay in milliseconds.
    pub(crate) static SCHEDULING_DELAY_MS: Event = Event::builder()
        .name("vicinal_scheduling_delay_ms")
        .histogram(SCHEDULING_DELAY_MS_BUCKETS)
        .build();

    /// Event for observing the execution time of tasks.
    ///
    /// The magnitude is the execution time in milliseconds.
    pub(crate) static EXECUTION_TIME_MS: Event = Event::builder()
        .name("vicinal_execution_time_ms")
        .histogram(EXECUTION_TIME_MS_BUCKETS)
        .build();
}
