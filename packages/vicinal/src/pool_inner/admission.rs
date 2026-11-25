//! Submission ownership, policy selection and queue admission.

use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
#[cfg(test)]
use std::thread::JoinHandle as ThreadJoinHandle;

use event_listener::IntoNotification;
use tracing::trace;

#[cfg(test)]
use crate::SubmissionTarget;
use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, NEVER_POISONED, PoolInner, WorkerGroupState, discard_task_after_failure,
};

impl PoolInner {
    /// Holds caller-owned state outside fallible preparation and poisoned rejection.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn submit<T, R>(&self, task: T, action: impl FnOnce(&mut Option<T>) -> R) -> R {
        let mut task = Some(task);
        let result = catch_unwind(AssertUnwindSafe(|| {
            assert!(!self.runtime.is_poisoned());
            action(&mut task)
        }));

        match result {
            Ok(result) => result,
            Err(payload) => {
                let already_stopped = self.runtime.is_shutdown() && self.runtime.is_poisoned();

                self.stop_admission();
                discard_task_after_failure(task);

                if already_stopped {
                    resume_unwind(payload);
                }

                self.poison(payload);
            }
        }
    }

    /// Admits a prepared task directly to policy-selected group storage.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        state: &WorkerGroupState,
        task: ErasedTaskHandle,
        urgent: bool,
    ) {
        let queue = if urgent {
            &state.urgent_queue
        } else {
            &state.regular_queue
        };
        let mut task = Some(task);
        let admitted = catch_unwind(AssertUnwindSafe(|| {
            let mut queue = queue.lock().expect(NEVER_POISONED);

            if self.runtime.is_poisoned() {
                drop(queue);
                panic!("worker pool is poisoned");
            }
            if self.runtime.is_shutdown() {
                return ControlFlow::Break(());
            }

            queue.push_back(task.take().expect("admission consumes each task once"));
            ControlFlow::Continue(())
        }));

        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err(payload) => {
                self.stop_admission();
                if let Some(task) = task.take() {
                    discard_task_after_failure(task);
                }
                self.poison(payload);
            }
        };

        match admitted {
            ControlFlow::Continue(()) => {
                state.record_task_spawned();
                state.wake_event.notify(1.additional());
                trace!(
                    target: POOL_LOG_TARGET,
                    pool_id = self.pool_id,
                    urgent,
                    "spawned task"
                );
            }
            ControlFlow::Break(()) => {
                trace!(
                    target: POOL_LOG_TARGET,
                    pool_id = self.pool_id,
                    "abandoned task during shutdown"
                );
            }
        }
    }

    /// Adapts controlled worker startup into the production admission path for race fixtures.
    #[cfg(test)]
    pub(crate) fn enqueue_with(
        self: &Arc<Self>,
        target: SubmissionTarget,
        state: &WorkerGroupState,
        task: ErasedTaskHandle,
        urgent: bool,
        start: impl FnOnce(&mut Vec<ThreadJoinHandle<()>>),
    ) {
        let mut task = Some(task);
        if self.runtime.is_shutdown() {
            self.enqueue(
                state,
                task.take()
                    .expect("a shutdown admission consumes its task once"),
                urgent,
            );
            return;
        }

        let mut started = Vec::new();
        let result = catch_unwind(AssertUnwindSafe(|| start(&mut started)));
        self.worker_handles
            .lock()
            .expect(NEVER_POISONED)
            .append(&mut started);
        // Shutdown may drain the fixture handles before the injected callback returns its
        // newly started handle. Publish then join any late handles to close that ownership gap.
        if self.runtime.is_shutdown() {
            let handles = std::mem::take(&mut *self.worker_handles.lock().expect(NEVER_POISONED));
            for handle in handles {
                _ = handle.join();
            }
        }
        if let Err(payload) = result {
            self.stop_admission();
            discard_task_after_failure(
                task.take()
                    .expect("failed startup retains its unadmitted task"),
            );
            self.poison(payload);
        }

        let selected = self.registry.group_id(target.group);
        let selected = self.registry.resolve(selected);
        assert!(std::ptr::eq(state, selected));
        self.enqueue(
            state,
            task.take()
                .expect("successful startup consumes its task once"),
            urgent,
        );
    }
}
