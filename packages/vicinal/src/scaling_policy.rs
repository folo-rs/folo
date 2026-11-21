use crate::ScalingState;

/// Decides whether a saturated locality group may start one additional worker.
///
/// Install an application-owned policy with
/// [`PoolBuilder::scaling_policy`][crate::PoolBuilder::scaling_policy].
/// Vicinal provides no built-in scaling algorithm. Without a policy, worker counts stay fixed.
///
/// Policies run only with queued work, all existing workers busy and the group below its
/// [`WorkerPolicy`][crate::WorkerPolicy] ceiling. Starting workers count as available capacity,
/// so their initialization does not trigger additional growth. A group with no workers qualifies.
/// Growth is checked on submission and when a worker becomes busy with more work still queued.
/// The latter also handles bursts submitted while a worker is starting.
///
/// Calls for one group are serialized; calls for different groups may overlap. The snapshot
/// can become stale while the policy runs, so approval does not guarantee a worker will start.
/// Each approval permits at most one worker. Workers remain alive until pool shutdown.
///
/// # Callback safety
///
/// The callback runs on submitting or worker threads, outside queue and registration locks.
/// It must not panic, must return promptly, must not submit to or shut down its pool, and
/// must not wait for same-pool work. It must preserve the calling thread's affinity because a
/// worker may execute its next task immediately after the callback returns. Policy invocation
/// has no dedicated unwind boundary.
pub trait ScalingPolicy: Send + Sync + 'static {
    /// Returns whether to start one additional worker.
    ///
    /// Returning `false` leaves tasks queued. In particular, refusing the first worker prevents
    /// execution until later demand causes a subsequent decision that obtains approval.
    /// Implementations must not panic.
    fn start_worker(&self, state: &ScalingState) -> bool;
}
