//! Submission ownership, policy selection and queue admission.

use std::panic::{self, AssertUnwindSafe};
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
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
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
                    panic::resume_unwind(payload);
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
        let accepted = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut queue = queue.lock().expect(NEVER_POISONED);

            if self.runtime.is_poisoned() {
                drop(queue);
                panic!("worker pool is poisoned");
            }
            if self.runtime.is_shutdown() {
                return false;
            }

            queue.push_back(task.take().expect("admission consumes each task once"));
            true
        }));

        let accepted = match accepted {
            Ok(accepted) => accepted,
            Err(payload) => {
                self.stop_admission();
                if let Some(task) = task.take() {
                    discard_task_after_failure(task);
                }
                self.poison(payload);
            }
        };

        if accepted {
            state.record_task_spawned();
            state.wake_event.notify(1.additional());
            trace!(
                target: POOL_LOG_TARGET,
                pool_id = self.pool_id,
                urgent,
                "spawned task"
            );
        } else {
            trace!(
                target: POOL_LOG_TARGET,
                pool_id = self.pool_id,
                "abandoned task during shutdown"
            );
        }
    }

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
        let result = panic::catch_unwind(AssertUnwindSafe(|| start(&mut started)));
        self.worker_handles
            .lock()
            .expect(NEVER_POISONED)
            .append(&mut started);
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
