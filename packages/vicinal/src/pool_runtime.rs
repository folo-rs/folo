//! Pool-wide terminal state shared by every policy-created worker group.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use events_once::EventLake;
use tracing::error;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, NEVER_POISONED, ThreadInit, WorkerGroupState, discard_panic,
    extract_panic_message,
};

pub(crate) struct PoolRuntime {
    pool_id: u64,
    pool_name: String,
    registry_owner: OnceLock<u64>,
    thread_init: Option<ThreadInit>,
    shutdown: AtomicBool,
    poisoned: AtomicBool,
    groups: Mutex<Vec<NonNull<WorkerGroupState>>>,
    workers: Mutex<WorkerLifecycle>,
    workers_changed: Condvar,
    failure: Mutex<Option<Box<dyn Any + Send>>>,
    pub(crate) result_channel_pool: EventLake,
}

impl PoolRuntime {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        thread_init: Option<ThreadInit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool_id,
            pool_name,
            registry_owner: OnceLock::new(),
            thread_init,
            shutdown: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            groups: Mutex::new(Vec::new()),
            workers: Mutex::new(WorkerLifecycle { active: 0 }),
            workers_changed: Condvar::new(),
            failure: Mutex::new(None),
            result_channel_pool: EventLake::new(),
        })
    }

    pub(crate) fn set_registry_owner(&self, owner: u64) {
        self.registry_owner
            .set(owner)
            .expect("one registry belongs to each pool runtime");
    }

    pub(crate) fn registry_owner(&self) -> u64 {
        *self
            .registry_owner
            .get()
            .expect("the pool registry is installed before groups are created")
    }

    pub(crate) fn pool_id(&self) -> u64 {
        self.pool_id
    }

    pub(crate) fn pool_name(&self) -> &str {
        &self.pool_name
    }

    pub(crate) fn thread_init(&self) -> Option<&ThreadInit> {
        self.thread_init.as_ref()
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    pub(crate) fn register_group(&self, group: &WorkerGroupState) {
        let mut groups = self.groups.lock().expect(NEVER_POISONED);
        assert!(
            !self.is_poisoned(),
            "worker groups cannot be added after pool failure"
        );

        groups.push(NonNull::from(group));
        group.mark_registered();
        if self.is_shutdown() {
            group.signal_shutdown();
        }
    }

    pub(crate) fn unregister_group(&self, group: &WorkerGroupState) {
        if !group.take_registered() {
            return;
        }

        let mut groups = self.groups.lock().expect(NEVER_POISONED);
        let address = NonNull::from(group);
        let index = groups
            .iter()
            .position(|candidate| *candidate == address)
            .expect("every live worker group is registered with its pool");
        groups.swap_remove(index);
    }

    pub(crate) fn begin_worker(&self) -> bool {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        if self.is_shutdown() {
            return false;
        }

        workers.active = workers
            .active
            .checked_add(1)
            .expect("worker count cannot exceed addressable memory");
        true
    }

    pub(crate) fn end_worker(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.active = workers
            .active
            .checked_sub(1)
            .expect("only registered workers can finish");
        self.workers_changed.notify_all();
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn stop_workers(&self) {
        let groups = self.groups.lock().expect(NEVER_POISONED);
        self.shutdown.store(true, Ordering::Release);

        for group in groups.iter() {
            // Holding the group registry lock prevents group destruction while these
            // addresses are borrowed.
            // SAFETY: The group registry lock prevents unregistering and dropping this
            // address for the entire borrow.
            let group = unsafe { group.as_ref() };
            group.signal_shutdown();
            group.notify_initialization_failure();
            #[cfg(test)]
            group.mark_startup_failed_if_started();
        }
    }

    pub(crate) fn stop_admission(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.stop_workers();
    }

    pub(crate) fn wait_for_workers(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        while workers.active != 0 {
            workers = self.workers_changed.wait(workers).expect(NEVER_POISONED);
        }
    }

    #[cfg(test)]
    pub(crate) fn active_workers(&self) -> usize {
        self.workers.lock().expect(NEVER_POISONED).active
    }

    pub(crate) fn fail(&self, payload: Box<dyn Any + Send>) {
        {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            if failure.is_none() {
                error!(
                    target: POOL_LOG_TARGET,
                    pool_id = self.pool_id,
                    message = extract_panic_message(&payload),
                    "worker pool infrastructure failed"
                );
                *failure = Some(payload);
            } else {
                // Keep the first infrastructure failure so later cleanup panics do not
                // replace the root cause.
                drop(failure);
                discard_panic(payload);
                return;
            }
        }

        self.stop_admission();
        self.drain_groups();
    }

    fn drain_groups(&self) {
        let groups = self.groups.lock().expect(NEVER_POISONED).clone();
        for group in groups {
            // Registered groups remain alive through the pool that owns this runtime.
            // SAFETY: The pool or an active worker owns every registered Arc group while
            // failure cleanup runs, and the registered address is never relocated.
            let group = unsafe { group.as_ref() };
            drain_queue(&group.urgent_queue);
            drain_queue(&group.regular_queue);
        }
    }

    pub(crate) fn resume_failure(&self) -> ! {
        let payload = self
            .failure
            .lock()
            .expect(NEVER_POISONED)
            .take()
            .expect("poisoned pool retains its original infrastructure failure");
        panic::resume_unwind(payload)
    }

    pub(crate) fn take_failure(&self) -> Option<Box<dyn Any + Send>> {
        self.failure.lock().expect(NEVER_POISONED).take()
    }
}

// SAFETY: The runtime stores stable addresses only while holding the registry's
// group-lifetime lock. Each address remains allocated until its registration is
// removed under that same lock.
unsafe impl Send for PoolRuntime {}
// SAFETY: Every shared state access is synchronized; raw pointers are dereferenced only
// while their registration and group lifetime are protected.
unsafe impl Sync for PoolRuntime {}

struct WorkerLifecycle {
    active: usize,
}

fn drain_queue(queue: &Mutex<std::collections::VecDeque<ErasedTaskHandle>>) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else {
            return;
        };

        if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| drop(task))) {
            error!(
                message = extract_panic_message(&payload),
                "failure during worker pool cleanup"
            );
            discard_panic(payload);
        }
    }
}
