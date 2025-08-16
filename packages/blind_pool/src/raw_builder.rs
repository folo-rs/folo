use crate::{DropPolicy, RawBlindPool};

/// Builder for creating an instance of [`RawBlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// Creates a raw pool for manual resource management.
///
/// # Examples
///
/// ```
/// use blind_pool::{RawBlindPool, DropPolicy};
///
/// // Default raw blind pool.
/// let mut pool = RawBlindPool::builder().build();
///
/// // With custom drop policy.
/// let mut pool = RawBlindPool::builder()
///     .drop_policy(DropPolicy::MustNotDropItems)
///     .build();
/// ```
#[derive(Debug)]
#[must_use]
pub struct RawBlindPoolBuilder {
    drop_policy: DropPolicy,
}

impl RawBlindPoolBuilder {
    pub(crate) fn new() -> Self {
        Self {
            drop_policy: DropPolicy::default(),
        }
    }

    /// Sets the [drop policy][DropPolicy] for the pool. This governs how
    /// to treat remaining items in the pool when the pool is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::{RawBlindPool, DropPolicy};
    ///
    /// let mut pool = RawBlindPool::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// Builds a raw pool with the specified configuration.
    ///
    /// This creates a [`RawBlindPool`] for manual resource management.
    /// Use this when you need direct control over item lifetimes.
    ///
    /// For simple cases, prefer [`RawBlindPool::new()`] which is equivalent to
    /// `RawBlindPool::builder().build()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::builder().build();
    /// let pooled_item = pool.insert(42_u32);
    ///
    /// // Manual cleanup required.
    /// pool.remove(pooled_item);
    /// ```
    #[must_use]
    pub fn build(self) -> RawBlindPool {
        RawBlindPool::new_inner(self.drop_policy)
    }
}
