//! Worker thread logic for executing tasks from queues.

use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::queue::SegQueue;
use infinity_pool::BlindPooledMut;

use crate::VicinalTask;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IterationResult {
    ExecutedUrgent,
    ExecutedRegular,
    Shutdown,
    WaitingForWork,
}

pub(crate) struct WorkerCore<'a> {
    urgent_queue: &'a SegQueue<BlindPooledMut<dyn VicinalTask>>,
    regular_queue: &'a SegQueue<BlindPooledMut<dyn VicinalTask>>,
    shutdown_flag: &'a AtomicBool,
}

impl<'a> WorkerCore<'a> {
    pub(crate) fn new(
        urgent_queue: &'a SegQueue<BlindPooledMut<dyn VicinalTask>>,
        regular_queue: &'a SegQueue<BlindPooledMut<dyn VicinalTask>>,
        shutdown_flag: &'a AtomicBool,
    ) -> Self {
        Self {
            urgent_queue,
            regular_queue,
            shutdown_flag,
        }
    }

    pub(crate) fn run_one_iteration(&self) -> IterationResult {
        if let Some(mut task) = self.urgent_queue.pop() {
            task.as_pin_mut().call();
            return IterationResult::ExecutedUrgent;
        }

        // Acquire ordering synchronizes with Release in signal_shutdown, ensuring we see
        // the latest value of the shutdown flag after all prior writes from the signaler.
        if self.shutdown_flag.load(Ordering::Acquire) {
            return IterationResult::Shutdown;
        }

        if let Some(mut task) = self.regular_queue.pop() {
            task.as_pin_mut().call();
            return IterationResult::ExecutedRegular;
        }

        IterationResult::WaitingForWork
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::AtomicU32;

    use infinity_pool::BlindPool;

    use super::*;
    use crate::PooledCastVicinalTask;

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
        let urgent = SegQueue::new();
        let regular = SegQueue::new();
        let shutdown = AtomicBool::new(false);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::WaitingForWork);
    }

    #[test]
    fn empty_queues_with_shutdown_returns_shutdown() {
        let urgent = SegQueue::new();
        let regular = SegQueue::new();
        let shutdown = AtomicBool::new(true);

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::Shutdown);
    }

    #[test]
    fn urgent_task_executes_and_returns_executed_urgent() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let pool = BlindPool::new();
        let urgent = SegQueue::new();
        let regular = SegQueue::new();
        let shutdown = AtomicBool::new(false);

        let task = pool.insert(CountingTask::new(&COUNTER));
        urgent.push(task.cast_vicinal_task());

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

        let pool = BlindPool::new();
        let urgent = SegQueue::new();
        let regular = SegQueue::new();
        let shutdown = AtomicBool::new(false);

        let urgent_task = pool.insert(CountingTask::new(&URGENT_COUNTER));
        let regular_task = pool.insert(CountingTask::new(&REGULAR_COUNTER));
        urgent.push(urgent_task.cast_vicinal_task());
        regular.push(regular_task.cast_vicinal_task());

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

        let pool = BlindPool::new();
        let urgent = SegQueue::new();
        let regular = SegQueue::new();
        let shutdown = AtomicBool::new(false);

        let task = pool.insert(CountingTask::new(&COUNTER));
        regular.push(task.cast_vicinal_task());

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        assert_eq!(core.run_one_iteration(), IterationResult::ExecutedRegular);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn shutdown_prevents_regular_task_execution() {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER.store(0, Ordering::Relaxed);

        let pool = BlindPool::new();
        let urgent = SegQueue::new();
        let regular = SegQueue::new();
        let shutdown = AtomicBool::new(true);

        let task = pool.insert(CountingTask::new(&COUNTER));
        regular.push(task.cast_vicinal_task());

        let core = WorkerCore::new(&urgent, &regular, &shutdown);

        // Shutdown takes priority over regular tasks.
        assert_eq!(core.run_one_iteration(), IterationResult::Shutdown);
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
    }
}
