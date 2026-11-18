//! Saturation-only growth admission, separate from fixed-count scheduling.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use many_cpus::ProcessorSet;

use crate::{GroupId, NEVER_POISONED, PoolInner, ScalingState, WorkerGroup};

impl PoolInner {
    /// Avoids queue inspection, policy dispatch and registration locking when capacity is available.
    pub(crate) fn grow_if_busy(self: &Arc<Self>, destination: GroupId, state: &WorkerGroup) {
        if !self.can_grow(state)
            || state
                .growing
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        let growth = GrowthGuard(&state.growing);
        if !self.can_grow(state) {
            return;
        }

        let workers = state.worker_count.load(Ordering::Acquire);
        // Concurrent completion may reduce outstanding work before publishing an idle worker.
        let queued = state
            .outstanding_tasks
            .load(Ordering::Acquire)
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
            let index = state.worker_count.fetch_add(1, Ordering::AcqRel);
            // Count the reservation before releasing growth admission. A new worker can then
            // grow again when it becomes busy, even before its creator returns from startup.
            drop(growth);
            drop(self.start_worker_on(
                destination,
                index,
                &affinity,
                &mut handles,
                thread::Builder::spawn,
                ProcessorSet::pin_current_thread_to,
            ));
        }));
        if result.is_err() {
            self.stop_admission();
        }
        drop(handles);
        result.unwrap_or_else(|payload| panic::resume_unwind(payload));
    }

    fn can_grow(&self, state: &WorkerGroup) -> bool {
        let workers = state.worker_count.load(Ordering::Acquire);
        workers < self.policy.count().get()
            && state.outstanding_tasks.load(Ordering::Acquire)
                > usize::try_from(workers).expect("worker counts fit the supported pointer widths")
            && state.busy_workers.load(Ordering::Acquire) == workers
            && !self.shutdown.load(Ordering::Acquire)
    }
}

struct GrowthGuard<'a>(&'a AtomicBool);

impl Drop for GrowthGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
