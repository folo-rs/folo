//! Per-processor state for worker threads.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam::queue::SegQueue;
use event_listener::Event;
use events_once::EventLake;
use infinity_pool::{BlindPool, BlindPooledMut};

use crate::VicinalTask;

pub(crate) struct ProcessorState {
    /// Queue for high-priority tasks. These are executed before regular tasks.
    pub(crate) urgent_queue: SegQueue<BlindPooledMut<dyn VicinalTask>>,

    /// Queue for normal-priority tasks.
    pub(crate) regular_queue: SegQueue<BlindPooledMut<dyn VicinalTask>>,

    /// Event used to wake up sleeping workers when new tasks are added.
    pub(crate) wake_event: Event,

    /// Flag indicating that the processor (and its workers) should shut down.
    pub(crate) shutdown_flag: AtomicBool,

    /// Flag indicating whether workers have been spawned for this processor.
    /// Used for lazy initialization of workers.
    pub(crate) workers_spawned: AtomicBool,

    /// Pool for storing task objects to avoid repeated allocation.
    pub(crate) task_pool: BlindPool,

    /// Pool for storing oneshot channels used to return task results.
    pub(crate) result_channel_pool: EventLake,

    /// Counter of total tasks spawned on this processor.
    pub(crate) tasks_spawned: AtomicU64,
}

impl ProcessorState {
    pub(crate) fn new() -> Self {
        Self {
            urgent_queue: SegQueue::new(),
            regular_queue: SegQueue::new(),
            wake_event: Event::new(),
            shutdown_flag: AtomicBool::new(false),
            workers_spawned: AtomicBool::new(false),
            task_pool: BlindPool::new(),
            result_channel_pool: EventLake::new(),
            tasks_spawned: AtomicU64::new(0),
        }
    }

    pub(crate) fn signal_shutdown(&self) {
        // Release ordering ensures all prior task queue operations are visible to workers
        // before they observe the shutdown flag.
        self.shutdown_flag.store(true, Ordering::Release);
        self.wake_event.notify(usize::MAX);
    }

    pub(crate) fn record_task_spawned(&self) {
        // Relaxed ordering is sufficient for a monotonic counter with no synchronization needs.
        self.tasks_spawned.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn get_tasks_spawned(&self) -> u64 {
        // Relaxed ordering is sufficient - no synchronization with other operations needed.
        self.tasks_spawned.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_queues() {
        let state = ProcessorState::new();

        assert!(state.urgent_queue.pop().is_none());
        assert!(state.regular_queue.pop().is_none());
    }

    #[test]
    fn shutdown_flag_initially_false() {
        let state = ProcessorState::new();

        assert!(!state.shutdown_flag.load(Ordering::Acquire));
    }

    #[test]
    fn signal_shutdown_sets_flag() {
        let state = ProcessorState::new();

        state.signal_shutdown();

        assert!(state.shutdown_flag.load(Ordering::Acquire));
    }

    #[test]
    fn tasks_spawned_counter_initially_zero() {
        let state = ProcessorState::new();

        assert_eq!(state.get_tasks_spawned(), 0);
    }

    #[test]
    fn record_task_spawned_increments_counter() {
        let state = ProcessorState::new();

        state.record_task_spawned();
        state.record_task_spawned();
        state.record_task_spawned();

        assert_eq!(state.get_tasks_spawned(), 3);
    }
}
