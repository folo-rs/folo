use std::marker::PhantomData;

use crate::{DropPolicy, RawBlindPool};

/// Creates an instance of [`RawBlindPool`].
#[derive(Debug, Default)]
#[must_use]
pub struct RawBlindPoolBuilder {
    drop_policy: DropPolicy,

    _not_send: PhantomData<*const ()>,
}

impl RawBlindPoolBuilder {
    /// Creates a new instance of the builder with the default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Defines the drop policy for the pool. Optional.
    pub fn drop_policy(mut self, drop_policy: DropPolicy) -> Self {
        self.drop_policy = drop_policy;
        self
    }

    /// Validates the options and creates the pool.
    #[must_use]
    pub fn build(self) -> RawBlindPool {
        RawBlindPool::new_inner(self.drop_policy)
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(RawBlindPoolBuilder: Send, Sync);

    #[test]
    fn builder_creates_functional_pool() {
        let pool = RawBlindPoolBuilder::new().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity_for::<u32>(), 0);
    }

    #[test]
    fn builder_default_works() {
        let pool = RawBlindPoolBuilder::default().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity_for::<String>(), 0);
    }

    #[test]
    fn builder_with_may_drop_contents_policy() {
        // Test that MayDropContents allows pool to be dropped with items still in it
        let mut pool = RawBlindPoolBuilder::new()
            .drop_policy(DropPolicy::MayDropContents)
            .build();

        // Insert some items and deliberately DO NOT remove them
        let _handle1 = pool.insert("test1".to_string());
        let _handle2 = pool.insert(42_u32);

        assert_eq!(pool.len(), 2);

        // Pool should be dropped successfully even with items still in it
        // (This is the whole point of MayDropContents policy)
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn builder_with_must_not_drop_contents_policy() {
        // Test that MustNotDropContents panics when pool is dropped with items
        let mut pool = RawBlindPoolBuilder::new()
            .drop_policy(DropPolicy::MustNotDropContents)
            .build();

        // Insert an item and deliberately DO NOT remove it
        let _handle = pool.insert(100_i64);
        assert_eq!(pool.len(), 1);

        // Pool should panic when dropped with items still in it
        // (This is the whole point of MustNotDropContents policy)
        drop(pool);
    }
}
