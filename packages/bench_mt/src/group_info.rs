use std::num::NonZero;

/// Informs a benchmark run callback which thread group it is currently executing for,
/// out of how many total.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupInfo {
    /// The index of the current thread group, starting from 0.
    index: usize,

    /// The total number of thread groups in the run.
    count: NonZero<usize>,
}

impl GroupInfo {
    /// Creates a new `GroupInfo` with the specified index and total count.
    pub(crate) fn new(index: usize, count: NonZero<usize>) -> Self {
        Self { index, count }
    }

    /// The index of the current thread group, starting from 0.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The total number of thread groups in the run.
    #[must_use]
    pub fn count(&self) -> NonZero<usize> {
        self.count
    }
}
