//! Shared pool ownership, policy binding and terminal cleanup.

use std::any::{Any, type_name};
use std::fmt::{Debug, Formatter, Result as FmtResult};
#[cfg(test)]
use std::io;
#[cfg(test)]
use std::num::NonZero;
#[cfg(test)]
use std::ops::Deref;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::thread::{Builder, JoinHandle};
use std::thread::{Result as ThreadResult, panicking};

use many_cpus::{ProcessorSet, SystemHardware};
use tracing::error;

use crate::{
    BindContext, BoundWorkerPolicy, BuildError, PoolRuntime, ThreadInit, WorkerGroups,
    WorkerPolicyBinding, discard_panic, extract_panic_message, report_diagnostic,
    validate_eager_processors,
};
#[cfg(test)]
use crate::{SubmissionTarget, WorkerEntry};

pub(crate) const POOL_LOG_TARGET: &str = module_path!();

/// Shares the runtime-owned pool name with tests that inspect the inner facade directly.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PoolName(Arc<str>);

#[cfg(test)]
impl Deref for PoolName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl PartialEq<&str> for PoolName {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_ref() == *other
    }
}

/// Keeps schedulers' shared storage alive while the owning pool controls worker lifetime.
pub(crate) struct PoolInner {
    pub(crate) pool_id: u64,
    #[cfg(test)]
    pub(crate) pool_name: PoolName,
    pub(crate) groups: WorkerGroups,
    #[cfg(test)]
    pub(crate) registry: WorkerGroups,
    #[cfg(test)]
    pub(crate) hardware: SystemHardware,
    #[cfg(test)]
    // Test fixtures own these OS-thread handles independently of pool participation.
    pub(crate) worker_handles: Mutex<Vec<JoinHandle<()>>>,
    pub(crate) policy: Box<dyn BoundWorkerPolicy>,
    pub(crate) runtime: Arc<PoolRuntime>,
}

impl PoolInner {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        hardware: &SystemHardware,
        policy: Box<dyn WorkerPolicyBinding>,
        thread_init: Option<ThreadInit>,
        eager: Option<&ProcessorSet>,
    ) -> Result<Self, BuildError> {
        let runtime = PoolRuntime::new(pool_id, pool_name, thread_init);
        let groups = WorkerGroups::new(Arc::clone(&runtime));
        if let Some(processors) = eager {
            validate_eager_processors(hardware, processors);
        }
        runtime.begin_worker_binding();
        let result = catch_unwind(AssertUnwindSafe(|| {
            policy.bind(BindContext::new(
                hardware,
                eager,
                &groups,
                runtime.pool_name(),
            ))
        }));
        runtime.end_worker_binding();

        let bound = match result {
            Ok(bound) => bound,
            Err(payload) => {
                runtime.fail(payload);
                finish_failed_binding(&runtime);
                return Err(runtime_build_error(&runtime)
                    .expect("runtime.fail retained the binding panic before cleanup"));
            }
        };
        let policy = match bound {
            Ok(policy) => policy,
            Err(error) => {
                finish_failed_binding(&runtime);
                return Err(runtime_build_error(&runtime).unwrap_or_else(|| error.into()));
            }
        };
        runtime.wait_for_binding_workers();
        // Custom binding can return after a participating worker independently records
        // a failure. Construction must propagate that published failure rather than returning
        // an already-poisoned pool.
        if runtime.is_poisoned() {
            runtime.wait_for_workers();
            return Err(runtime_build_error(&runtime)
                .expect("poisoned construction retains the first infrastructure failure"));
        }

        Ok(Self {
            pool_id,
            #[cfg(test)]
            pool_name: PoolName(runtime.pool_name_snapshot()),
            groups: groups.clone(),
            #[cfg(test)]
            registry: groups,
            #[cfg(test)]
            hardware: hardware.clone(),
            #[cfg(test)]
            worker_handles: Mutex::new(Vec::new()),
            policy,
            runtime,
        })
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
        self.runtime
            .drain_groups(|result| record_failure(&mut failure, result, &self.runtime));
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
        self.runtime
            .drain_groups(|result| record_failure(&mut failure, result, &self.runtime));
        self.runtime.wait_for_workers();

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
}

/// Stops policy-owned workers and waits for every binding reservation before returning failure.
///
/// A panic during the consumed policy's binding or destruction is contained by the caller's
/// unwind boundary before this helper runs. Both payload and ordinary error paths therefore
/// share the same worker cleanup barrier.
fn finish_failed_binding(runtime: &PoolRuntime) {
    runtime.stop_workers();
    runtime.wait_for_binding_workers();
    runtime.wait_for_workers();
}

/// Converts the first retained construction panic without rethrowing it.
fn runtime_build_error(runtime: &PoolRuntime) -> Option<BuildError> {
    runtime.take_failure().map(BuildError::from_panic)
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl Debug for PoolInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("pool_id", &self.pool_id)
            .field("pool_name", &self.runtime.pool_name())
            .field("policy", &self.policy)
            .finish_non_exhaustive()
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
