//! Worker thread logic for executing tasks from queues.

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, Ordering};

use crate::{ErasedTaskHandle, NEVER_POISONED, discard_task_after_failure};

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
/// It borrows only the parts of a group's state that executing work needs, which lets a
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

    #[cfg(test)]
    pub(crate) fn run_one_iteration(&self) -> IterationResult {
        self.run_one_iteration_with(|| true)
    }

    /// Refreshes thread-local state and confirms admission outside the queue lock after dequeueing.
    // Mutating worker-exit decisions can strand independently scheduled lifecycle tests.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn run_one_iteration_with(
        &self,
        before_task: impl FnOnce() -> bool,
    ) -> IterationResult {
        // We first check with Relaxed to minimize overhead, as this will be called often.
        if self.shutdown_flag.load(Ordering::Relaxed) {
            // Acquire ordering synchronizes with Release in signal_shutdown, ensuring we see
            // the latest value of the shutdown flag after all prior writes from the signaler.
            atomic::fence(Ordering::Acquire);

            return IterationResult::Shutdown;
        }

        let task = self.urgent_queue.lock().expect(NEVER_POISONED).pop_front();
        if let Some(task) = task {
            if !execute_task(task, before_task) {
                return IterationResult::Shutdown;
            }
            return IterationResult::ExecutedUrgent;
        }

        let task = self.regular_queue.lock().expect(NEVER_POISONED).pop_front();
        if let Some(task) = task {
            if !execute_task(task, before_task) {
                return IterationResult::Shutdown;
            }
            return IterationResult::ExecutedRegular;
        }

        IterationResult::WaitingForWork
    }
}

/// Keeps preparation/execution infrastructure and capture destruction in separate
/// unwind boundaries.
// Returning the wrong execution outcome can strand lifecycle tests after all workers exit.
#[cfg_attr(test, mutants::skip)]
fn execute_task(mut task: ErasedTaskHandle, before_task: impl FnOnce() -> bool) -> bool {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        if before_task() {
            task.as_pin_mut().call();
            true
        } else {
            false
        }
    })) {
        Ok(executed) => executed,
        Err(payload) => {
            discard_task_after_failure(task);
            panic::resume_unwind(payload);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::AtomicU32;

    use plurality::MultiPool;
    use testing::assert_panics;

    use super::*;
    use crate::metrics::CLOCK;
    use crate::pool_inner::{OnDrop, PanickingPayload};
    use crate::{VicinalTask, init_task, wrap_task_and_forget};

    fn alloc_task(pool: &MultiPool, task: impl VicinalTask) -> ErasedTaskHandle {
        init_task(pool.alloc_uninit_box(), task)
    }

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
    fn boundary_shutdown_abandons_dequeued_task_outside_queue_lock() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);
        let pool = MultiPool::new();
        for queue in [&regular, &urgent] {
            queue
                .lock()
                .unwrap()
                .push_back(alloc_task(&pool, CountingTask::new(&COUNTER)));
            let core = WorkerCore::new(&urgent, &regular, &shutdown);
            assert_eq!(
                core.run_one_iteration_with(|| {
                    drop(urgent.try_lock().unwrap());
                    drop(regular.try_lock().unwrap());
                    false
                }),
                IterationResult::Shutdown
            );
            assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
            assert!(pool.is_empty());
        }
    }

    /// Preparation failure must survive a capture that also fails while being abandoned.
    #[test]
    fn boundary_failure_contains_panicking_capture_and_payload() {
        struct AffinityFailure;

        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);
        let pool = MultiPool::new();
        for queue in [&urgent, &regular] {
            queue.lock().unwrap().push_back(alloc_task(
                &pool,
                OnDrop(Some(Box::new(|| panic::panic_any(PanickingPayload)))),
            ));
            let core = WorkerCore::new(&urgent, &regular, &shutdown);
            let payload = panic::catch_unwind(AssertUnwindSafe(|| {
                core.run_one_iteration_with(|| {
                    drop(urgent.try_lock().unwrap());
                    drop(regular.try_lock().unwrap());
                    panic::panic_any(AffinityFailure);
                })
            }))
            .unwrap_err();
            assert!(payload.is::<AffinityFailure>());
            assert!(queue.lock().unwrap().is_empty());
            assert!(pool.is_empty());
        }
    }

    /// A metric failure before invoking user code must leave captures available for safe disposal.
    #[test]
    fn task_preparation_failure_contains_panicking_capture() {
        let pool = MultiPool::new();
        let capture = OnDrop(Some(Box::new(|| panic::panic_any(PanickingPayload))));
        let wrapped = wrap_task_and_forget(
            move || drop(capture),
            CLOCK.with_borrow_mut(fast_time::Clock::now),
        );
        let task = alloc_task(&pool, wrapped);
        CLOCK.with_borrow_mut(|_| assert_panics(|| execute_task(task, || true)));
        assert!(pool.is_empty());
    }

    #[test]
    fn urgent_task_executes_and_returns_executed_urgent() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let pool = MultiPool::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let task = alloc_task(&pool, CountingTask::new(&COUNTER));
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

        let pool = MultiPool::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let urgent_task = alloc_task(&pool, CountingTask::new(&URGENT_COUNTER));
        let regular_task = alloc_task(&pool, CountingTask::new(&REGULAR_COUNTER));
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

        let pool = MultiPool::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(false);

        let task = alloc_task(&pool, CountingTask::new(&COUNTER));
        regular.lock().unwrap().push_back(task);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::ExecutedRegular);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn shutdown_prevents_regular_task_execution() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let pool = MultiPool::new();
        let urgent = Mutex::new(VecDeque::new());
        let regular = Mutex::new(VecDeque::new());
        let shutdown = AtomicBool::new(true);

        let task = alloc_task(&pool, CountingTask::new(&COUNTER));
        regular.lock().unwrap().push_back(task);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        // Shutdown takes priority over regular tasks.
        assert_eq!(core.run_one_iteration(), IterationResult::Shutdown);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
    }
}
