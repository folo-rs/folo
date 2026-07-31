//! Worker thread logic for executing tasks from queues.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, Ordering};

use crate::{ErasedTaskHandle, NEVER_POISONED};

/// What one pass of a worker's main loop accomplished, and thus what the loop does next.
///
/// The two `Executed` outcomes mean work was found and the loop should immediately look for
/// more; they are kept apart so tests can assert queue priority. `WaitingForWork` sends the
/// worker to sleep on the wake event and `Shutdown` ends the thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IterationResult {
    ExecutedUrgent,
    ExecutedRegular,
    Shutdown,
    WaitingForWork,
}

/// The body of a worker thread's main loop, isolated from thread management.
///
/// It borrows only the parts of a processor's state that executing work needs, which lets a
/// single iteration be driven directly by tests without spawning a thread or constructing a
/// pool. Priority between the two queues lives here and nowhere else.
pub(crate) struct WorkerCore<'a> {
    urgent_queue: &'a Mutex<VecDeque<ErasedTaskHandle>>,
    regular_queue: &'a Mutex<VecDeque<ErasedTaskHandle>>,
    shutdown_flag: &'a AtomicBool,
}

impl<'a> WorkerCore<'a> {
    pub(crate) fn new(
        urgent_queue: &'a Mutex<VecDeque<ErasedTaskHandle>>,
        regular_queue: &'a Mutex<VecDeque<ErasedTaskHandle>>,
        shutdown_flag: &'a AtomicBool,
    ) -> Self {
        Self {
            urgent_queue,
            regular_queue,
            shutdown_flag,
        }
    }

    pub(crate) fn run_one_iteration(&self) -> IterationResult {
        // We first check with Relaxed to minimize overhead, as this will be called often.
        if self.shutdown_flag.load(Ordering::Relaxed) {
            // Acquire ordering synchronizes with Release in signal_shutdown, ensuring we see
            // the latest value of the shutdown flag after all prior writes from the signaler.
            atomic::fence(Ordering::Acquire);

            return IterationResult::Shutdown;
        }

        let task = self.urgent_queue.lock().expect(NEVER_POISONED).pop_front();
        if let Some(mut task) = task {
            task.as_mut().call();
            return IterationResult::ExecutedUrgent;
        }

        let task = self.regular_queue.lock().expect(NEVER_POISONED).pop_front();
        if let Some(mut task) = task {
            task.as_mut().call();
            return IterationResult::ExecutedRegular;
        }

        IterationResult::WaitingForWork
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::AtomicU32;

    use multitude::Arena;

    use super::*;
    use crate::{VicinalTask, erase_task};

    /// A simple task that increments a counter when called.
    struct CountingTask {
        counter: &'static AtomicU32,
        called: bool,
    }

    impl CountingTask {
        fn new(counter: &'static AtomicU32) -> Self {
            Self {
                counter,
                called: false,
            }
        }
    }

    impl VicinalTask for CountingTask {
        fn call(self: Pin<&mut Self>) {
            let this = self.get_mut();
            if !this.called {
                this.called = true;
                this.counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[test]
    fn empty_queues_no_shutdown_returns_waiting() {
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::WaitingForWork);
    }

    #[test]
    fn empty_queues_with_shutdown_returns_shutdown() {
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(true);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::Shutdown);
    }

    #[test]
    fn urgent_task_executes_and_returns_executed_urgent() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let arena = Arena::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let task = erase_task(&arena, CountingTask::new(&COUNTER));
        urgent.lock().unwrap().push_back(task);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::ExecutedUrgent);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn urgent_takes_priority_over_regular() {
        static URGENT_COUNTER: AtomicU32 = AtomicU32::new(0);
        static REGULAR_COUNTER: AtomicU32 = AtomicU32::new(0);
        URGENT_COUNTER.store(0, Ordering::Relaxed);
        REGULAR_COUNTER.store(0, Ordering::Relaxed);

        let arena = Arena::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let urgent_task = erase_task(&arena, CountingTask::new(&URGENT_COUNTER));
        let regular_task = erase_task(&arena, CountingTask::new(&REGULAR_COUNTER));
        urgent.lock().unwrap().push_back(urgent_task);
        regular.lock().unwrap().push_back(regular_task);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        // First iteration should execute urgent task only.
        assert_eq!(core.run_one_iteration(), IterationResult::ExecutedUrgent);
        assert_eq!(URGENT_COUNTER.load(Ordering::Relaxed), 1);
        assert_eq!(REGULAR_COUNTER.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn regular_task_executes_when_urgent_empty() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let arena = Arena::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let task = erase_task(&arena, CountingTask::new(&COUNTER));
        regular.lock().unwrap().push_back(task);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::ExecutedRegular);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn shutdown_prevents_regular_task_execution() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let arena = Arena::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(true);

        let task = erase_task(&arena, CountingTask::new(&COUNTER));
        regular.lock().unwrap().push_back(task);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        // Shutdown takes priority over regular tasks.
        assert_eq!(core.run_one_iteration(), IterationResult::Shutdown);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
    }
}
