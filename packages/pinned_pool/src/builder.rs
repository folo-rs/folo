use std::marker::PhantomData;

use crate::{DropPolicy, PinnedPool};

/// Builder for creating an instance of [`PinnedPool`].
///
/// You only need to use this builder if you want to customize the pool configuration.
/// The default configuration used by [`PinnedPool::new()`][1] is sufficient for most use cases.
///
/// # Examples
///
/// ```
/// use pinned_pool::{DropPolicy, PinnedPool};
///
/// let pool = PinnedPool::<u32>::builder()
///     .drop_policy(DropPolicy::MayDropItems)
///     .build();
/// ```
///
/// [1]: PinnedPool::new
#[must_use]
pub struct PinnedPoolBuilder<T> {
    drop_policy: DropPolicy,

    _item: PhantomData<T>,
}

impl<T> std::fmt::Debug for PinnedPoolBuilder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedPoolBuilder")
            .field(
                "item_type",
                &std::format_args!("{}", std::any::type_name::<T>()),
            )
            .field("drop_policy", &self.drop_policy)
            .finish()
    }
}

impl<T> PinnedPoolBuilder<T> {
    pub(crate) fn new() -> Self {
        Self {
            drop_policy: DropPolicy::default(),
            _item: PhantomData,
        }
    }

    /// Sets the [drop policy][DropPolicy] for the pool. This governs how
    /// to treat remaining items in the pool when the pool is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use pinned_pool::{DropPolicy, PinnedPool};
    ///
    /// let pool = PinnedPool::<u32>::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// Builds the pinned pool with the specified configuration.
    ///
    /// # Panics
    ///
    /// Panics if `T` is zero-sized.
    ///
    /// # Examples
    ///
    /// ```
    /// use pinned_pool::PinnedPool;
    ///
    /// let pool = PinnedPool::<u32>::builder().build();
    /// ```
    #[must_use]
    pub fn build(self) -> PinnedPool<T> {
        PinnedPool::new_inner(self.drop_policy)
    }
}
