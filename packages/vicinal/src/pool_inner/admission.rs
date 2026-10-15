//! Submission ownership, cold startup transactions and queue admission.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle as ThreadJoinHandle};

use event_listener::IntoNotification;
use many_cpus::{ProcessorId, ProcessorSet};
use tracing::trace;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, NEVER_POISONED, PoolInner, SubmissionTarget, WorkerGroup,
    discard_task_after_failure,
};

impl PoolInner {
    /// Holds caller-owned state outside fallible preparation and poisoned rejection.
    // Missing failure cleanup can strand accepted work.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn submit<T, R>(&self, task: T, action: impl FnOnce(&mut Option<T>) -> R) -> R {
        let mut task = Some(task);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            assert!(!self.poisoned.load(Ordering::Acquire));
            action(&mut task)
        }));

        match result {
            Ok(result) => result,
            Err(payload) => {
                let already_stopped =
                    self.shutdown.load(Ordering::Acquire) && self.poisoned.load(Ordering::Acquire);

                // Destructors can re-enter submission, so reject work before releasing captures.
                self.stop_admission();
                discard_task_after_failure(task);

                if already_stopped {
                    panic::resume_unwind(payload);
                }

                self.poison(payload);
            }
        }
    }

    /// Starts and admits work as one cold transaction; established submitters use only a queue.
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: ErasedTaskHandle,
        urgent: bool,
    ) {
        self.enqueue_with(target, state, task, urgent, |handles| {
            self.start_workers(
                target,
                handles,
                thread::Builder::spawn,
                ProcessorSet::pin_current_thread_to,
            );
        });
    }

    /// Separates the startup adapter for deterministic failure and registration-race tests.
    // Broken startup/registration can strand workers or submissions.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn enqueue_with(
        self: &Arc<Self>,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: ErasedTaskHandle,
        urgent: bool,
        start: impl FnOnce(&mut Vec<ThreadJoinHandle<()>>),
    ) {
        // Keep task ownership outside all guards so rejection never drops captures under a lock.
        let mut task = Some(task);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let known_processor = !self.policy.shares_processors()
                || self.membership(target.processor).load(Ordering::Acquire);

            if known_processor && state.startup.get().is_some() {
                self.admit(target, state, &mut task, urgent, None)
            } else {
                self.admit_cold(target, state, &mut task, urgent, start)
            }
        }));

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(payload) => {
                self.stop_admission();
                if let Some(task) = task.take() {
                    discard_task_after_failure(task);
                }

                self.poison(payload);
            }
        };

        match outcome {
            Admission::Accepted { expanded } => {
                if expanded {
                    state.wake_event.notify(usize::MAX);
                }

                state.record_task_spawned();
                // Each queued task may block its worker; do not coalesce wakeups.
                state.wake_event.notify(1.additional());
                trace!(
                    target: POOL_LOG_TARGET,
                    pool_id = self.pool_id,
                    group = ?target.group,
                    urgent,
                    "spawned task"
                );
            }
            Admission::Shutdown => {
                trace!(
                    target: POOL_LOG_TARGET,
                    pool_id = self.pool_id,
                    group = ?target.group,
                    "abandoned task during shutdown"
                );
            }
        }
    }

    /// Serializes first admission and membership expansion with worker registration and shutdown.
    // Broken startup/registration can strand workers or submissions.
    #[cfg_attr(test, mutants::skip)]
    fn admit_cold(
        &self,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: &mut Option<ErasedTaskHandle>,
        urgent: bool,
        start: impl FnOnce(&mut Vec<ThreadJoinHandle<()>>),
    ) -> Admission {
        // No other submitter can overtake startup's first admission while this lock is held.
        let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            assert!(!self.poisoned.load(Ordering::Acquire));
            if self.shutdown.load(Ordering::Acquire) {
                return Admission::Shutdown;
            }

            ensure_started(state, &mut handles, start);
            let affinity = self.expanded_affinity(target);
            self.admit(target, state, task, urgent, affinity)
        }));

        // Publish failure before releasing registration, but resume unwinding outside the lock.
        if result.is_err() {
            self.stop_admission();
        }
        drop(handles);

        match result {
            Ok(outcome) => outcome,
            Err(payload) => panic::resume_unwind(payload),
        }
    }

    /// Builds a shared mask only when the current submitter adds a processor to its group.
    // Missing expansion can strand revision-sensitive workers.
    #[cfg_attr(test, mutants::skip)]
    fn expanded_affinity(&self, target: SubmissionTarget) -> Option<Arc<ProcessorSet>> {
        if !self.policy.shares_processors()
            || self.membership(target.processor).load(Ordering::Acquire)
        {
            return None;
        }

        let affinity = self
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| {
                target.group.contains(processor)
                    && (processor.id() == target.processor
                        || self.membership(processor.id()).load(Ordering::Acquire))
            })
            .take_all()
            .expect("the sampled submitting processor belongs to its group");

        Some(Arc::new(affinity))
    }

    /// Publishes membership only with an accepted task, under its queue's admission lock.
    // Accepting after shutdown/failure strands joined handles.
    #[cfg_attr(test, mutants::skip)]
    fn admit(
        &self,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: &mut Option<ErasedTaskHandle>,
        urgent: bool,
        affinity: Option<Arc<ProcessorSet>>,
    ) -> Admission {
        let queue = if urgent {
            &state.urgent_queue
        } else {
            &state.regular_queue
        };
        let mut queue = queue.lock().expect(NEVER_POISONED);

        if self.poisoned.load(Ordering::Acquire) {
            drop(queue);
            panic!("worker pool is poisoned");
        }

        if self.shutdown.load(Ordering::Acquire) {
            return Admission::Shutdown;
        }

        queue.push_back(task.take().expect("admission consumes each task once"));
        let expanded = affinity.is_some();

        if let Some(affinity) = affinity {
            let mut current = state.affinity.lock().expect(NEVER_POISONED);
            *current = Some(affinity);

            // Revisions cannot overflow: each CPU enters this fixed topology at most once.
            state.affinity_revision.fetch_add(1, Ordering::Release);
            self.membership(target.processor)
                .store(true, Ordering::Release);
        }
        drop(queue);

        Admission::Accepted { expanded }
    }

    /// Resolves a populated CPU without allocating or locking on established submissions.
    pub(crate) fn membership(&self, processor: ProcessorId) -> &AtomicBool {
        let index = self
            .member_processors
            .binary_search_by_key(&processor, |(id, _)| *id)
            .expect("a submitting processor must belong to the eligible topology");
        let (_, member) = self
            .member_processors
            .get(index)
            .expect("binary search returns an existing membership slot");

        member
    }
}

/// Distinguishes rejected work from accepted work that may also need a group-wide wakeup.
enum Admission {
    Shutdown,
    Accepted { expanded: bool },
}

/// Publishes readiness or permanent startup failure while the caller owns registration.
// Broken readiness publication can strand workers or submissions.
#[cfg_attr(test, mutants::skip)]
fn ensure_started(
    state: &WorkerGroup,
    handles: &mut Vec<ThreadJoinHandle<()>>,
    start: impl FnOnce(&mut Vec<ThreadJoinHandle<()>>),
) {
    if state.startup.get().is_some() {
        return;
    }

    PoolInner::start_group(state, || start(handles));
}
