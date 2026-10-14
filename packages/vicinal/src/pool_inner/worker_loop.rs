//! Worker-side affinity refresh and wake coordination around individual task execution.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use event_listener::{Listener, listener};
use many_cpus::ProcessorSet;
use tracing::trace;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{GroupId, IterationResult, NEVER_POISONED, PoolInner, WorkerCore, WorkerGroup};

/// Drives `WorkerCore` with this group's queues and wake signal.
// Broken wakeup coordination can strand workers indefinitely.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn worker_loop(
    inner: &PoolInner,
    destination: GroupId,
    worker_index: u32,
    pin: impl Fn(&ProcessorSet),
) {
    let state = inner.registry.get_or_init(destination);
    let core = WorkerCore::new(
        &state.urgent_queue,
        &state.regular_queue,
        &state.shutdown_flag,
    );
    let mut revision = 0;
    let shared = inner.policy.shares_processors();

    loop {
        if inner.shutdown.load(Ordering::Acquire) {
            break;
        }

        if shared {
            refresh_affinity(state, &mut revision, &pin);
        }

        let outcome = core.run_one_iteration_with(
            || {
                if inner.shutdown.load(Ordering::Acquire) {
                    return false;
                }

                if shared {
                    refresh_affinity(state, &mut revision, &pin);
                }

                !inner.shutdown.load(Ordering::Acquire)
            },
            || inner.stop_admission(),
        );

        match outcome {
            IterationResult::ExecutedUrgent | IterationResult::ExecutedRegular => {
                trace!(
                    target: POOL_LOG_TARGET,
                    pool_id = inner.pool_id,
                    group = ?destination,
                    worker_index,
                    "executed task"
                );
            }
            IterationResult::Shutdown => break,
            IterationResult::WaitingForWork => {
                listener!(state.wake_event => listener);

                // Changes between the executor's empty observation and listener registration
                // cannot notify this listener. Recheck after registration to cover that gap.
                if !state.urgent_queue.lock().expect(NEVER_POISONED).is_empty()
                    || !state.regular_queue.lock().expect(NEVER_POISONED).is_empty()
                    || state.shutdown_flag.load(Ordering::Acquire)
                    || state.affinity_revision.load(Ordering::Acquire) != revision
                {
                    continue;
                }

                #[cfg(test)]
                if let Some(before_wait) = state.before_wait.get() {
                    before_wait(worker_index, revision);
                }

                listener.wait();
            }
        }
    }
}

/// Applies a monotonically published snapshot on the owning worker, never under a shared lock.
#[cfg_attr(test, mutants::skip)] // Missed revisions can strand idle workers or accepted work.
pub(crate) fn refresh_affinity(
    state: &WorkerGroup,
    revision: &mut usize,
    pin: impl FnOnce(&ProcessorSet),
) {
    if state.affinity_revision.load(Ordering::Acquire) == *revision {
        return;
    }

    let (affinity, latest) = {
        let affinity = state.affinity.lock().expect(NEVER_POISONED);
        (
            Arc::clone(
                affinity
                    .as_ref()
                    .expect("a revision publishes a nonempty mask"),
            ),
            state.affinity_revision.load(Ordering::Acquire),
        )
    };

    pin(&affinity);
    *revision = latest;
}
