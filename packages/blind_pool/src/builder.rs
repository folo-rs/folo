use crate::{BlindPool, DropPolicy, LocalBlindPool, RawBlindPool};

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

/// Builder for creating an instance of [`LocalBlindPool`].
///
/// This builder allows configuration of pool behavior before creation.
/// Creates a single-threaded blind pool with automatic resource management.
///
/// # Examples
///
/// ```
/// use blind_pool::{LocalBlindPool, DropPolicy};
///
/// // Default single-threaded blind pool.
/// let pool = LocalBlindPool::builder().build();
///
/// // With custom drop policy.
/// let pool = LocalBlindPool::builder()
///     .drop_policy(DropPolicy::MustNotDropItems)
///     .build();
/// ```
#[derive(Debug)]
#[must_use]
pub struct LocalBlindPoolBuilder {
    drop_policy: DropPolicy,
}

impl LocalBlindPoolBuilder {
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
    /// use blind_pool::{LocalBlindPool, DropPolicy};
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
