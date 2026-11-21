use std::num::NonZero;

/// A point-in-time view of one saturated locality group, supplied to [`crate::ScalingPolicy`].
///
/// The group has queued work, every existing worker is busy, and its worker count is below
/// the maximum. Concurrent task execution and submission can change these values.
#[derive(Clone, Debug)]
pub struct ScalingState {
    workers: u32,
    max_workers: NonZero<u32>,
    queued_tasks: usize,
}

impl ScalingState {
    pub(crate) fn new(workers: u32, max_workers: NonZero<u32>, queued_tasks: usize) -> Self {
        Self {
            workers,
            max_workers,
            queued_tasks,
        }
    }

    /// Returns the number of workers already started in this group.
    #[must_use]
    pub fn workers(&self) -> u32 {
        self.workers
    }

    /// Returns this group's hard worker ceiling, configured through [`crate::WorkerPolicy`].
    #[must_use]
    pub fn max_workers(&self) -> u32 {
        self.max_workers.get()
    }

    /// Returns the number of queued tasks, including regular and urgent work.
    #[must_use]
    pub fn queued_tasks(&self) -> usize {
        self.queued_tasks
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(ScalingState: Clone, Send, Sync, UnwindSafe, RefUnwindSafe);
}
