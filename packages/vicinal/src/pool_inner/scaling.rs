//! Saturation-only growth admission, separate from fixed-count scheduling.

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread::{self, JoinHandle as ThreadJoinHandle};

use many_cpus::ProcessorSet;

use crate::pool_inner::WorkerEntry;
use crate::worker_group::{GROWTH_IDLE, GROWTH_PENDING, GROWTH_RUNNING};
use crate::{GroupId, NEVER_POISONED, PoolInner, ScalingState, WorkerGroup};

impl PoolInner {
    /// Avoids queue inspection, policy dispatch and registration locking when capacity is available.
    pub(crate) fn grow_if_busy(self: &Arc<Self>, destination: GroupId, state: &WorkerGroup) {
        self.grow_if_busy_with(
            destination,
            state,
            thread::Builder::spawn,
            ProcessorSet::pin_current_thread_to,
        );
    }

    /// Separates worker startup adapters for deterministic growth-failure tests.
    #[cfg_attr(test, mutants::skip)] // Missing cleanup can strand accepted work.
    pub(crate) fn grow_if_busy_with(
        self: &Arc<Self>,
        destination: GroupId,
        state: &WorkerGroup,
        mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        if !self.can_grow(state) {
            return;
        }

        let Some(mut growth) = GrowthGuard::acquire(&state.growth_state) else {
            return;
        };

        loop {
            self.try_grow_once(destination, state, &mut spawn, pin.clone());
            if !growth.finish_or_retry() {
                break;
            }
        }
    }

    fn try_grow_once(
        self: &Arc<Self>,
        destination: GroupId,
        state: &WorkerGroup,
        spawn: &mut impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Send + 'static,
    ) {
        if !self.can_grow(state) {
            return;
        }

        let workers = state.worker_count.load(Ordering::SeqCst);
        // Concurrent completion may reduce outstanding work before publishing an idle worker.
        let queued = state
            .outstanding_tasks
            .load(Ordering::SeqCst)
            .saturating_sub(
                usize::try_from(workers).expect("worker counts fit the supported pointer widths"),
            );
        if queued == 0 || !self.can_grow(state) {
            return;
        }

        let snapshot = ScalingState::new(workers, self.policy.count(), queued);
        let policy = self
            .scaling_policy
            .as_ref()
            .expect("only scaling pools request growth");
        if !(policy.0).start_worker(&snapshot) {
            return;
        }

        let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
        // Never unwind through the registration guard: cleanup must still be able to acquire it.
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            if !self.can_grow(state) {
                return;
            }
            let affinity = state
                .affinity
                .lock()
                .expect(NEVER_POISONED)
                .clone()
                .unwrap_or_else(|| {
                    Arc::new(
                        self.hardware
                            .all_processors()
                            .to_builder()
                            .filter(|processor| destination.contains(processor))
                            .take_all()
                            .expect("a processor destination contains its eligible processor"),
                    )
                });
            let index = state.worker_count.fetch_add(1, Ordering::SeqCst);
            drop(self.start_worker_on(destination, index, &affinity, &mut handles, spawn, pin));
        }));
        if result.is_err() {
            self.stop_admission();
        }
        drop(handles);
        if let Err(payload) = result {
            self.poison(payload);
        }
    }

    fn can_grow(&self, state: &WorkerGroup) -> bool {
        let workers = state.worker_count.load(Ordering::SeqCst);
        workers < self.policy.count().get()
            && state.outstanding_tasks.load(Ordering::SeqCst)
                > usize::try_from(workers).expect("worker counts fit the supported pointer widths")
            && state.busy_workers.load(Ordering::SeqCst) == workers
            && !self.shutdown.load(Ordering::Acquire)
    }
}

struct GrowthGuard<'a> {
    state: &'a AtomicU8,
    active: bool,
}

impl<'a> GrowthGuard<'a> {
    fn acquire(state: &'a AtomicU8) -> Option<Self> {
        loop {
            match state.compare_exchange(
                GROWTH_IDLE,
                GROWTH_RUNNING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Some(Self {
                        state,
                        active: true,
                    });
                }
                Err(GROWTH_RUNNING) => {
                    match state.compare_exchange(
                        GROWTH_RUNNING,
                        GROWTH_PENDING,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) | Err(GROWTH_PENDING) => return None,
                        Err(GROWTH_IDLE) => {}
                        Err(value) => {
                            unreachable!("growth state has an unknown value: {value}")
                        }
                    }
                }
                Err(GROWTH_PENDING) => return None,
                Err(value) => unreachable!("growth state has an unknown value: {value}"),
            }
        }
    }

    fn finish_or_retry(&mut self) -> bool {
        match self.state.compare_exchange(
            GROWTH_RUNNING,
            GROWTH_IDLE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => {
                self.active = false;
                false
            }
            Err(GROWTH_PENDING) => {
                self.state.store(GROWTH_RUNNING, Ordering::SeqCst);
                true
            }
            Err(value) => unreachable!("growth state has an unknown value: {value}"),
        }
    }
}

impl Drop for GrowthGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            self.state.store(GROWTH_IDLE, Ordering::SeqCst);
        }
    }
}
