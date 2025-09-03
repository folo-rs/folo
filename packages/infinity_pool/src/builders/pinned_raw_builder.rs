use std::marker::PhantomData;

use crate::{DropPolicy, RawPinnedPool};

/// Creates an instance of [`RawPinnedPool`].
#[derive(Debug)]
#[must_use]
pub struct RawPinnedPoolBuilder<T> {
    drop_policy: DropPolicy,

    _type: PhantomData<T>,

    _not_send: PhantomData<*const ()>,
}

impl<T> RawPinnedPoolBuilder<T> {
    /// Creates a new instance of the builder with the default options.
    pub fn new() -> Self {
        Self {
            drop_policy: DropPolicy::default(),
            _type: PhantomData,
            _not_send: PhantomData,
        }
    }

    /// Defines the drop policy for the pool. Optional.
    pub fn drop_policy(mut self, drop_policy: DropPolicy) -> Self {
        self.drop_policy = drop_policy;
        self
    }

    /// Validates the options and creates the pool.
    #[must_use]
    pub fn build(self) -> RawPinnedPool<T> {
        RawPinnedPool::new_inner(self.drop_policy)
    }
}

impl<T> Default for RawPinnedPoolBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(RawPinnedPoolBuilder<i32>: Send, Sync);

    #[test]
    fn builder_creates_functional_pool() {
        let pool = RawPinnedPoolBuilder::<u32>::new().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn builder_with_drop_policy() {
        let pool = RawPinnedPoolBuilder::<u32>::new()
            .drop_policy(DropPolicy::MustNotDropContents)
            .build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn builder_default_works() {
        let pool = RawPinnedPoolBuilder::<u32>::default().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn builder_via_pool_static_method() {
        let pool = RawPinnedPool::<f64>::builder().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }
}
