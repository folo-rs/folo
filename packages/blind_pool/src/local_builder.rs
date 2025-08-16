use std::cell::Cell;
use std::marker::PhantomData;

use crate::{DropPolicy, LocalBlindPool, RawBlindPool};

/// Builder for creating an instance of [`LocalBlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// Creates a single-threaded blind pool with automatic resource management.
///
/// # Examples
///
/// ```
/// use blind_pool::{DropPolicy, LocalBlindPool};
///
/// // Default single-threaded blind pool.
/// let pool = LocalBlindPool::builder().build();
///
/// // With custom drop policy.
/// let pool = LocalBlindPool::builder()
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
pub struct LocalBlindPoolBuilder {
    drop_policy: DropPolicy,

    // Prevents Sync while allowing Send - builders are thread-mobile but not thread-safe
    _not_sync: PhantomData<Cell<()>>,
}

impl LocalBlindPoolBuilder {
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
    /// use blind_pool::{DropPolicy, LocalBlindPool};
    ///
    /// let pool = LocalBlindPool::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// Builds a single-threaded blind pool with the specified configuration.
    ///
    /// This creates a [`LocalBlindPool`] with automatic resource management
    /// but no thread safety overhead.
    ///
    /// For simple cases, prefer [`LocalBlindPool::new()`] which is equivalent to
    /// `LocalBlindPool::builder().build()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::builder().build();
    /// let item = pool.insert(42_u32);
    /// assert_eq!(*item, 42);
    /// ```
    #[must_use]
    pub fn build(self) -> LocalBlindPool {
        LocalBlindPool::from(RawBlindPool::new_inner(self.drop_policy))
    }
}
