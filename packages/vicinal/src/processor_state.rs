//! Per-processor state for worker threads.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use event_listener::Event;
use events_once::EventLake;
use plurality::MultiPool;

use crate::ErasedTaskHandle;

/// Everything a processor's worker threads share: the work they draw from, the storage that
/// work lives in, and the signals that tell them to wake up or stop.
///
/// One instance exists per processor that has ever had a task spawned on it, and every
/// worker pinned to that processor operates on the same instance. Keeping the state
/// per-processor rather than per-pool is what gives spawned work its locality: a task is
/// queued on, allocated on, and executed on the processor that spawned it.
pub(crate) struct ProcessorState {
    /// Queue for high-priority tasks. These are executed before regular tasks.
    pub(crate) urgent_queue: Mutex<VecDeque<ErasedTaskHandle>>,

    /// Queue for normal-priority tasks.
    pub(crate) regular_queue: Mutex<VecDeque<ErasedTaskHandle>>,

    /// Event used to wake up sleeping workers when new tasks are added.
    pub(crate) wake_event: Event,

    /// Flag indicating that the processor (and its workers) should shut down.
    pub(crate) shutdown_flag: AtomicBool,

    /// Flag indicating whether workers have been spawned for this processor.
    /// Used for lazy initialization of workers.
    pub(crate) workers_spawned: AtomicBool,

    /// Backing storage for the task objects queued on this processor.
    ///
    /// `MultiPool` is not `Sync`, so the mutex serializes allocation by scheduler threads.
    /// Slot initialization, queueing, execution and release happen after this lock is released.
    pub(crate) task_pool: Mutex<MultiPool>,

    /// Pool for storing oneshot channels used to return task results.
    pub(crate) result_channel_pool: EventLake,

    /// Counter of total tasks spawned on this processor.
    pub(crate) tasks_spawned: AtomicU64,
}

impl ProcessorState {
    pub(crate) fn new() -> Self {
        Self {
            urgent_queue: Mutex::new(VecDeque::new()),
            regular_queue: Mutex::new(VecDeque::new()),
            wake_event: Event::new(),
            shutdown_flag: AtomicBool::new(false),
            workers_spawned: AtomicBool::new(false),
            task_pool: Mutex::new(MultiPool::new()),
            result_channel_pool: EventLake::new(),
            tasks_spawned: AtomicU64::new(0),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Removing this causes timeouts (workers never stop)
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::{VicinalTask, init_task};

    /// Spans multiple allocation/release operations without making Miri runs expensive.
    const CONCURRENT_TASK_COUNT: usize = 32;

    /// Minimal task used to exercise cross-thread slot release.
    struct NoopTask;

    impl VicinalTask for NoopTask {
        fn call(self: Pin<&mut Self>) {}
    }

    fn alloc_noop_task(state: &ProcessorState) -> ErasedTaskHandle {
        let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
        init_task(slot, NoopTask)
    }

    #[test]
    fn new_creates_empty_queues() {
        let state = ProcessorState::new();

        assert!(state.urgent_queue.lock().unwrap().pop_front().is_none());
        assert!(state.regular_queue.lock().unwrap().pop_front().is_none());
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

    #[test]
    fn task_slots_can_be_released_while_allocating() {
        let state = Arc::new(ProcessorState::new());
        let tasks = std::iter::repeat_with(|| alloc_noop_task(&state))
            .take(CONCURRENT_TASK_COUNT)
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(2));

        std::thread::scope(|scope| {
            scope.spawn({
                let barrier = Arc::clone(&barrier);
                move || {
                    barrier.wait();
                    drop(tasks);
                }
            });

            barrier.wait();
            for _ in 0..CONCURRENT_TASK_COUNT {
                drop(alloc_noop_task(&state));
            }
        });

        assert!(state.task_pool.lock().unwrap().is_empty());
    }
}
