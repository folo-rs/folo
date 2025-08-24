use std::cell::Cell;
use std::marker::PhantomData;

use crate::{DropPolicy, RawBlindPool};

/// Builder for creating an instance of [`RawBlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// Creates a raw pool for manual resource management.
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
///
/// # Thread safety
///
/// The builder is thread-mobile ([`Send`]) and can be safely transferred between threads,
/// allowing pool configuration to happen on different threads than where the pool is used.
/// However, it is not thread-safe ([`Sync`]) as it contains mutable configuration state.
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
    #[inline]
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
    /// let pooled_item = pool.insert("Test".to_string());
    ///
    /// // Manual cleanup required.
    /// unsafe { pool.remove(&pooled_item) };
    /// ```
    #[must_use]
    #[inline]
    pub fn build(self) -> RawBlindPool {
        RawBlindPool::new_inner(self.drop_policy)
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::RawBlindPoolBuilder;
    use crate::DropPolicy;

    #[test]
    fn thread_mobility_assertions() {
        // RawBlindPoolBuilder should be thread-mobile (Send) but not thread-safe (Sync)
        assert_impl_all!(RawBlindPoolBuilder: Send);
        assert_not_impl_any!(RawBlindPoolBuilder: Sync);
    }

    #[test]
    fn builder_default_configuration() {
        let builder = RawBlindPoolBuilder::new();
        let mut pool = builder.build();

        // Should work with default configuration
        let handle = pool.insert(42_u32);

        let value = *handle; // Safe deref access
        assert_eq!(value, 42);
        assert_eq!(pool.len(), 1);

        // SAFETY: This pooled handle is being consumed and cannot be used again.
        unsafe {
            pool.remove(&handle);
        }
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn builder_with_drop_policy_allow() {
        let mut pool = RawBlindPoolBuilder::new()
            .drop_policy(DropPolicy::MayDropItems)
            .build();

        // Should work normally
        let handle = pool.insert(42_u32);

        let value = *handle; // Safe deref access
        assert_eq!(value, 42);

        // Pool should be droppable even with items (doesn't panic)
        drop(pool); // Deliberately not removing the item
    }

    #[test]
    fn builder_with_drop_policy_must_not_drop() {
        let mut pool = RawBlindPoolBuilder::new()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // Should work normally
        let handle = pool.insert(42_u32);

        let value = *handle; // Safe deref access
        assert_eq!(value, 42);

        // Clean up properly - required for MustNotDropItems
        // SAFETY: This pooled handle is being consumed and cannot be used again.
        unsafe {
            pool.remove(&handle);
        }
        assert_eq!(pool.len(), 0);

        // Pool should be droppable when empty
        drop(pool);
    }
}
