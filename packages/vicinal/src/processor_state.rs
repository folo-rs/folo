//! Per-processor state for worker threads.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam::channel::{self, Receiver, Sender};
use events_once::EventLake;
use infinity_pool::{BlindPool, BlindPooledMut};
use parking_lot::RwLock;

use crate::VicinalTask;

pub(crate) struct ProcessorState {
    /// Channel for high-priority tasks. These are executed before regular tasks.
    /// Wrapped in `RwLock<Option>` to allow dropping the sender to signal shutdown.
    pub(crate) urgent_sender: RwLock<Option<Sender<BlindPooledMut<dyn VicinalTask>>>>,
    pub(crate) urgent_receiver: Receiver<BlindPooledMut<dyn VicinalTask>>,

    /// Channel for normal-priority tasks.
    pub(crate) regular_sender: RwLock<Option<Sender<BlindPooledMut<dyn VicinalTask>>>>,
    pub(crate) regular_receiver: Receiver<BlindPooledMut<dyn VicinalTask>>,

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
        let (urgent_sender, urgent_receiver) = channel::unbounded();
        let (regular_sender, regular_receiver) = channel::unbounded();

        Self {
            urgent_sender: RwLock::new(Some(urgent_sender)),
            urgent_receiver,
            regular_sender: RwLock::new(Some(regular_sender)),
            regular_receiver,
            shutdown_flag: AtomicBool::new(false),
            workers_spawned: AtomicBool::new(false),
            task_pool: BlindPool::new(),
            result_channel_pool: EventLake::new(),
            tasks_spawned: AtomicU64::new(0),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Removing this causes timeouts (workers never stop)
    pub(crate) fn signal_shutdown(&self) {
        // Release ordering ensures all prior task queue operations are visible to workers
        // before they observe the shutdown flag.
        self.shutdown_flag.store(true, Ordering::Release);

        // Drop senders to signal shutdown to workers.
        *self.urgent_sender.write() = None;
        *self.regular_sender.write() = None;
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

        assert!(state.urgent_receiver.is_empty());
        assert!(state.regular_receiver.is_empty());
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
