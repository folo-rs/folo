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
#[derive(Debug)]
#[must_use]
pub struct BlindPoolBuilder {
    drop_policy: DropPolicy,
}

impl BlindPoolBuilder {
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
