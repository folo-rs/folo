use crate::{BlindPool, DropPolicy};

/// Builder for creating an instance of [`BlindPool`].
///
/// Unlike [`opaque_pool::OpaquePoolBuilder`], this builder does not require specifying
/// a layout since [`BlindPool`] accepts objects of any layout.
///
/// # Examples
///
/// ```
/// use blind_pool::{BlindPool, DropPolicy};
///
/// // Basic blind pool.
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

    /// Builds the blind pool with the specified configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::builder().build();
    /// ```
    #[must_use]
    pub fn build(self) -> BlindPool {
        BlindPool::new_inner(self.drop_policy)
    }
}
