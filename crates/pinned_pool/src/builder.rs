use std::marker::PhantomData;

use crate::{DropPolicy, PinnedPool};

/// Builder for creating an instance of [`PinnedPool`].
#[derive(Debug)]
#[must_use]
pub struct PinnedPoolBuilder<T> {
    drop_policy: DropPolicy,

    _item: PhantomData<T>,
}

impl<T> PinnedPoolBuilder<T> {
    pub(crate) fn new() -> Self {
        Self {
            drop_policy: DropPolicy::default(),
            _item: PhantomData,
        }
    }

    /// Sets the drop policy for the pool.
    pub fn drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// Builds the pinned pool with the specified configuration.
    ///
    /// # Panics
    ///
    /// Panics if `T` is zero-sized.
    #[must_use]
    pub fn build(self) -> PinnedPool<T> {
        PinnedPool::new_inner(self.drop_policy)
    }
}
