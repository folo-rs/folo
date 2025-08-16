use std::cell::Cell;
use std::marker::PhantomData;

use crate::{DropPolicy, RawBlindPool};

/// Builder for creating an instance of [`RawBlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// Creates a raw pool for manual resource management.
///
/// The builder is thread-mobile ([`Send`]) and can be safely transferred between threads,
/// allowing pool configuration to happen on different threads than where the pool is used.
/// However, it is not thread-safe ([`Sync`]) as it contains mutable configuration state.
///
/// # Examples
///
/// ```
/// use blind_pool::{DropPolicy, RawBlindPool};
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
    // Prevents Sync while allowing Send - builders are thread-mobile but not thread-safe
    _not_sync: PhantomData<Cell<()>>,
}

impl RawBlindPoolBuilder {
    pub(crate) fn new() -> Self {
        Self {
            drop_policy: DropPolicy::default(),
            _not_sync: PhantomData,
        }
    }

    /// Sets the [drop policy][DropPolicy] for the pool. This governs how
    /// to treat remaining items in the pool when the pool is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::{DropPolicy, RawBlindPool};
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
