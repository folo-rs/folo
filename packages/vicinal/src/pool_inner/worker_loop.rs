//! Worker-side affinity refresh and wake coordination around individual task execution.

use std::sync::atomic::Ordering;

use event_listener::{Listener, listener};
use many_cpus::ProcessorSet;
use tracing::trace;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{IterationResult, NEVER_POISONED, WorkerCore, WorkerGroupState};

pub(crate) fn worker_loop_with(
    state: &WorkerGroupState,
    worker_index: u32,
    mut revision: usize,
    pin: impl FnMut(&ProcessorSet),
) {
    let runtime = state.runtime();
    let core = WorkerCore::new(
        &state.urgent_queue,
        &state.regular_queue,
        &state.shutdown_flag,
    );
    let mut pin = pin;

    loop {
        if runtime.is_shutdown() {
            break;
        }

        refresh_affinity(state, &mut revision, &mut pin);

        let outcome = core.run_one_iteration_with(
            || {
                if runtime.is_shutdown() {
                    return false;
                }

                refresh_affinity(state, &mut revision, &mut pin);

                !runtime.is_shutdown()
            },
            || runtime.stop_admission(),
        );

        match outcome {
            IterationResult::ExecutedUrgent | IterationResult::ExecutedRegular => {
                if let Some(group) = state.destination {
                    trace!(
                        target: POOL_LOG_TARGET,
                        pool_id = runtime.pool_id(),
                        group = ?group,
                        worker_index,
                        "executed task"
                    );
                } else {
                    trace!(
                        target: POOL_LOG_TARGET,
                        pool_id = runtime.pool_id(),
                        worker_index,
                        "executed task"
                    );
                }
            }
            IterationResult::Shutdown => break,
            IterationResult::WaitingForWork => {
                listener!(state.wake_event => listener);

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

#[cfg_attr(test, mutants::skip)]
pub(crate) fn refresh_affinity(
    state: &WorkerGroupState,
    revision: &mut usize,
    pin: impl FnOnce(&ProcessorSet),
) {
    if state.affinity_revision.load(Ordering::Acquire) == *revision {
        return;
    }

    let (affinity, latest) = {
        let affinity = state.affinity.read().expect(NEVER_POISONED);
        (
            affinity
                .as_ref()
                .expect("a revision publishes a nonempty mask")
                .clone(),
            state.affinity_revision.load(Ordering::Acquire),
        )
    };
    pin(&affinity);
    *revision = latest;
}
