use std::cell::Cell;
use std::marker::PhantomData;

use crate::{BlindPool, DropPolicy, RawBlindPool};

/// Builder for creating an instance of [`BlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// Creates a thread-safe blind pool with automatic resource management.
///
/// # Examples
///
/// ```
/// use blind_pool::{BlindPool, DropPolicy};
///
/// // Default thread-safe blind pool.
/// let pool = BlindPool::builder().build();
///
/// // With custom drop policy.
/// let pool = BlindPool::builder()
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
pub struct BlindPoolBuilder {
    drop_policy: DropPolicy,

    // Prevents Sync while allowing Send - builders are thread-mobile but not thread-safe
    _not_sync: PhantomData<Cell<()>>,
}

impl BlindPoolBuilder {
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
    /// use blind_pool::{BlindPool, DropPolicy};
    ///
    /// let pool = BlindPool::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// Builds a thread-safe blind pool with the specified configuration.
    ///
    /// This creates a [`BlindPool`] with automatic resource management and thread safety.
    ///
    /// For simple cases, prefer [`BlindPool::new()`] which is equivalent to
    /// `BlindPool::builder().build()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::builder().build();
    /// let item = pool.insert(42_u32);
    /// assert_eq!(*item, 42);
    /// ```
    #[must_use]
    pub fn build(self) -> BlindPool {
        BlindPool::from(RawBlindPool::new_inner(self.drop_policy))
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::BlindPoolBuilder;

    #[test]
    fn thread_mobility_assertions() {
        // BlindPoolBuilder should be thread-mobile (Send) but not thread-safe (Sync)
        assert_impl_all!(BlindPoolBuilder: Send);
        assert_not_impl_any!(BlindPoolBuilder: Sync);
    }
}
