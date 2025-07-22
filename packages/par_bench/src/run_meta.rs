use std::num::NonZero;

/// Informs a benchmark run callback about the basic metadata associated with the run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunMeta {
    /// The index of the current thread group, starting from 0.
    group_index: usize,

    /// The total number of thread groups in the run.
    group_count: NonZero<usize>,

    /// How many iterations will be executed as part of this run.
    iterations: u64,
}

impl RunMeta {
    /// Creates a new `RunMeta` with the specified index and total count.
    pub(crate) fn new(group_index: usize, group_count: NonZero<usize>, iterations: u64) -> Self {
        Self {
            group_index,
            group_count,
            iterations,
        }
    }

    /// The index of the current thread group, starting from 0.
    #[must_use]
    pub fn group_index(&self) -> usize {
        self.group_index
    }

    /// The total number of thread groups in the run.
    #[must_use]
    pub fn group_count(&self) -> NonZero<usize> {
        self.group_count
    }

    /// How many iterations will be executed as part of this run.
    #[must_use]
    pub fn iterations(&self) -> u64 {
        self.iterations
    }
}
