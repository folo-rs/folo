//! Shared pool ownership, policy preparation and terminal cleanup.

use std::any::{Any, type_name};
use std::collections::VecDeque;
#[cfg(test)]
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread::Result as ThreadResult;
use std::{fmt, thread};

use many_cpus::{ProcessorSet, SystemHardware};
use tracing::error;

use crate::{
    ErasedTaskHandle, NEVER_POISONED, PoolRuntime, ThreadInit, WorkerGroups, WorkerPolicy,
    WorkerPolicyContext, discard_panic, extract_panic_message, validate_eager_processors,
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
    pub(crate) worker_handles: Mutex<Vec<thread::JoinHandle<()>>>,
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
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            policy.prepare(WorkerPolicyContext::new(
                hardware, eager, &groups, &pool_name,
            ));
        }));

        if let Err(payload) = result {
            runtime.fail(payload);
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
        handles: &mut Vec<thread::JoinHandle<()>>,
        spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<thread::JoinHandle<()>>,
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
        self.policy
            .install_test_group(&self.registry, &group, &affinity);
        group.start_workers_with(
            destination,
            &affinity,
            self.policy.count(),
            handles,
            spawn,
            pin,
        );
    }

    #[cfg(test)]
    pub(crate) fn install_test_group(&self, target: SubmissionTarget) {
        let affinity = self
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| {
                processor.id() == target.processor && target.group.contains(processor)
            })
            .take_all()
            .unwrap_or_else(|| {
                panic!("no eligible processors for worker group {:?}", target.group)
            });
        let group = self.registry.group(target.group);
        self.policy
            .install_test_group(&self.registry, &group, &affinity);
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn poison(&self, payload: Box<dyn Any + Send>) -> ! {
        self.runtime.fail(payload);
        let mut failure = Err(self
            .runtime
            .take_failure()
            .expect("newly recorded infrastructure failure remains available"));
        self.stop_groups(&mut failure);
        panic::resume_unwind(failure.expect_err("pool failure retains its original panic"))
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn shutdown_and_wait_for_workers(&self) {
        self.runtime.stop_workers();

        let mut failure = self.runtime.take_failure().map_or(Ok(()), |payload| {
            error!(
                message = extract_panic_message(&payload),
                "failure during worker pool cleanup"
            );
            Err(payload)
        });
        self.stop_groups(&mut failure);
        self.runtime.wait_for_workers();

        #[cfg(test)]
        {
            let handles = std::mem::take(&mut *self.worker_handles.lock().expect(NEVER_POISONED));
            for handle in handles {
                record_failure(&mut failure, handle.join(), &self.runtime);
            }
        }

        if let Some(payload) = self.runtime.take_failure() {
            error!(
                message = extract_panic_message(&payload),
                "failure during worker pool cleanup"
            );
            if failure.is_ok() {
                failure = Err(payload);
            } else {
                discard_panic(payload);
            }
        }

        if let Err(payload) = failure {
            if thread::panicking() {
                error!(
                    pool_id = self.pool_id,
                    "worker or task cleanup panicked during unwinding"
                );
                discard_panic(payload);
            } else {
                panic::resume_unwind(payload);
            }
        }
    }

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
impl fmt::Debug for PoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            panic::catch_unwind(AssertUnwindSafe(|| drop(task))),
            runtime,
        );
    }
}

fn record_failure(first: &mut ThreadResult<()>, result: ThreadResult<()>, runtime: &PoolRuntime) {
    if let Err(payload) = result {
        runtime.stop_admission();
        error!(
            message = extract_panic_message(&payload),
            "failure during worker pool cleanup"
        );

        if first.is_ok() {
            *first = Err(payload);
        } else {
            error!("additional panic while shutting down worker pool");
            discard_panic(payload);
        }
    }
}
