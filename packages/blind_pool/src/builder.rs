use crate::{BlindPool, DropPolicy, LocalBlindPool, RawBlindPool};

/// Builder for creating an instance of [`BlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// By default, creates a thread-safe blind pool. Use `build_raw` for
/// manual resource management or `build_local` for single-threaded use.
///
/// # Examples
///
/// ```
/// use blind_pool::{BlindPool, DropPolicy};
///
/// // Default blind pool (thread-safe).
/// let pool = BlindPool::builder().build();
///
/// // Single-threaded blind pool.
/// let pool = BlindPool::builder().build_local();
///
/// // Raw pool for manual management.
/// let pool = BlindPool::builder().build_raw();
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
    /// This is the default build method that creates a [`BlindPool`] with
    /// automatic resource management and thread safety.
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
        BlindPool::from(self.build_raw())
    }

    /// Builds a single-threaded blind pool with the specified configuration.
    ///
    /// This creates a [`LocalBlindPool`] with automatic resource management
    /// but no thread safety overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::builder().build_local();
    /// let item = pool.insert(42_u32);
    /// assert_eq!(*item, 42);
    /// ```
    #[must_use]
    pub fn build_local(self) -> LocalBlindPool {
        LocalBlindPool::from(self.build_raw())
    }

    /// Builds a raw pool with the specified configuration.
    ///
    /// This creates a [`RawBlindPool`] for manual resource management.
    /// Use this when you need direct control over item lifetimes.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::builder().build_raw();
    /// let pooled_item = pool.insert(42_u32);
    ///
    /// // Manual cleanup required.
    /// pool.remove(pooled_item);
    /// ```
    #[must_use]
    pub fn build_raw(self) -> RawBlindPool {
        RawBlindPool::new_inner(self.drop_policy)
    }
}
