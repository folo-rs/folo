//! Shared pool ownership, policy preparation and terminal cleanup.

use std::any::{Any, type_name};
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter, Result as FmtResult};
#[cfg(test)]
use std::io;
#[cfg(test)]
use std::mem::take;
#[cfg(test)]
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::thread::{Builder, JoinHandle};
use std::thread::{Result as ThreadResult, panicking};

use many_cpus::{ProcessorSet, SystemHardware};
use tracing::error;

use crate::{
    ErasedTaskHandle, NEVER_POISONED, PoolRuntime, PrepareContext, ThreadInit, WorkerGroups,
    WorkerPolicy, discard_panic, extract_panic_message, report_diagnostic,
    validate_eager_processors,
};
#[cfg(test)]
use crate::{SubmissionTarget, WorkerEntry};

pub(crate) const POOL_LOG_TARGET: &str = module_path!();

/// Keeps schedulers' shared storage alive while the owning pool controls worker lifetime.
pub(crate) struct PoolInner {
    pub(crate) pool_id: u64,
    pub(crate) pool_name: String,
    pub(crate) groups: WorkerGroups,
    #[cfg(test)]
    pub(crate) registry: WorkerGroups,
    #[cfg(test)]
    pub(crate) hardware: SystemHardware,
    #[cfg(test)]
    pub(crate) worker_handles: Mutex<Vec<JoinHandle<()>>>,
    pub(crate) policy: Box<dyn WorkerPolicy>,
    pub(crate) runtime: Arc<PoolRuntime>,
}

impl PoolInner {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        hardware: &SystemHardware,
        mut policy: Box<dyn WorkerPolicy>,
        thread_init: Option<ThreadInit>,
        eager: Option<&ProcessorSet>,
    ) -> Self {
        let runtime = PoolRuntime::new(pool_id, pool_name.clone(), thread_init);
        let groups = WorkerGroups::new(Arc::clone(&runtime));
        if let Some(processors) = eager {
            validate_eager_processors(hardware, processors);
        }
        runtime.begin_worker_preparation();
        let result = catch_unwind(AssertUnwindSafe(|| {
            policy.prepare(PrepareContext::new(hardware, eager, &groups, &pool_name));
        }));
        runtime.end_worker_preparation();

        if let Err(payload) = result {
            runtime.fail(payload);
            runtime.wait_for_workers();
            runtime.resume_failure();
        }
        runtime.wait_for_preparation_workers();
        // Custom preparation can return after a participating worker independently records
        // a failure. Construction must propagate that published failure rather than returning
        // an already-poisoned pool.
        if runtime.is_poisoned() {
            runtime.wait_for_workers();
            runtime.resume_failure();
        }

        Self {
            pool_id,
            pool_name,
            groups: groups.clone(),
            #[cfg(test)]
            registry: groups,
            #[cfg(test)]
            hardware: hardware.clone(),
            #[cfg(test)]
            worker_handles: Mutex::new(Vec::new()),
            policy,
            runtime,
        }
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn stop_admission(&self) {
        self.runtime.stop_admission();
    }

    #[cfg(test)]
    pub(crate) fn start_workers(
        self: &Arc<Self>,
        target: SubmissionTarget,
        count: NonZero<u32>,
        handles: &mut Vec<JoinHandle<()>>,
        spawn: impl FnMut(Builder, WorkerEntry) -> io::Result<JoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let destination = target.group;
        let affinity = self
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| {
                processor.id() == target.processor && destination.contains(processor)
            })
            .take_all()
            .unwrap_or_else(|| panic!("no eligible processors for worker group {destination:?}"));
        let group = self.registry.group(destination);
        group.start_workers_with(destination, &affinity, count, handles, spawn, pin);
    }

    /// Terminates admission for a submission-owned failure and resumes its original payload.
    ///
    /// Ref: docs/implementation.md, "Task storage" and "Affinity and failure coordination".
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn poison(&self, payload: Box<dyn Any + Send>) -> ! {
        let payload = self.runtime.fail_for_submission(payload);
        let mut failure = Err(payload);
        self.stop_groups(&mut failure);
        resume_unwind(failure.expect_err("pool failure retains its original panic"))
    }

    /// Publishes shutdown, contains queue cleanup and waits for all participating workers.
    ///
    /// This owns pool destruction cleanup; policy-owned operating-system threads may perform
    /// cleanup after leaving [`Worker::run()`][crate::Worker::run].
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn shutdown_and_wait_for_workers(&self) {
        self.runtime.stop_workers();

        let mut failure = self.runtime.take_failure().map_or(Ok(()), |payload| {
            report_diagnostic(|| {
                error!(
                    message = extract_panic_message(&payload),
                    "failure during worker pool cleanup"
                );
            });
            Err(payload)
        });
        self.stop_groups(&mut failure);
        self.runtime.wait_for_workers();

        #[cfg(test)]
        {
            let handles = take(&mut *self.worker_handles.lock().expect(NEVER_POISONED));
            for handle in handles {
                record_failure(&mut failure, handle.join(), &self.runtime);
            }
        }

        if let Some(payload) = self.runtime.take_failure() {
            report_diagnostic(|| {
                error!(
                    message = extract_panic_message(&payload),
                    "failure during worker pool cleanup"
                );
            });
            if failure.is_ok() {
                failure = Err(payload);
            } else {
                discard_panic(payload);
            }
        }

        if let Err(payload) = failure {
            if panicking() {
                report_diagnostic(|| {
                    error!(
                        pool_id = self.pool_id,
                        "worker or task cleanup panicked during unwinding"
                    );
                });
                discard_panic(payload);
            } else {
                resume_unwind(payload);
            }
        }
    }

    /// Signals all groups before draining their queues outside registry and queue locks.
    #[cfg_attr(test, mutants::skip)]
    fn stop_groups(&self, failure: &mut ThreadResult<()>) {
        let groups = self.groups.snapshot();
        for group in &groups {
            group.state().signal_shutdown();
        }
        for group in groups {
            discard_queue(&group.state().urgent_queue, failure, &self.runtime);
            discard_queue(&group.state().regular_queue, failure, &self.runtime);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl Debug for PoolInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("pool_id", &self.pool_id)
            .field("pool_name", &self.pool_name)
            .finish_non_exhaustive()
    }
}

/// Drains the live queue without running user destructors under a lock.
#[cfg_attr(test, mutants::skip)]
fn discard_queue(
    queue: &Mutex<VecDeque<ErasedTaskHandle>>,
    failure: &mut ThreadResult<()>,
    runtime: &PoolRuntime,
) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else {
            break;
        };

        record_failure(
            failure,
            catch_unwind(AssertUnwindSafe(|| drop(task))),
            runtime,
        );
    }
}

/// Records the first cleanup failure and preserves later diagnostics without interrupting cleanup.
fn record_failure(first: &mut ThreadResult<()>, result: ThreadResult<()>, runtime: &PoolRuntime) {
    if let Err(payload) = result {
        runtime.stop_admission();
        report_diagnostic(|| {
            error!(
                message = extract_panic_message(&payload),
                "failure during worker pool cleanup"
            );
        });

        if first.is_ok() {
            *first = Err(payload);
        } else {
            report_diagnostic(|| error!("additional panic while shutting down worker pool"));
            discard_panic(payload);
        }
    }
}
