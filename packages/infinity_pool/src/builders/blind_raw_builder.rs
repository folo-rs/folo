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
        let mut pool = RawBlindPoolBuilder::new()
            .drop_policy(DropPolicy::MayDropContents)
            .build();

        // Insert some items to test drop behavior
        let handle1 = pool.insert("test1".to_string());
        let handle2 = pool.insert(42_u32);
        
        assert_eq!(pool.len(), 2);
        
        // Verify items are accessible
        assert_eq!(&*handle1, "test1");
        assert_eq!(*handle2, 42);
        
        // Remove items explicitly
        // SAFETY: Handles are valid and from this pool
        unsafe {
            pool.remove(handle1.into_shared());
        }
        // SAFETY: Handle is valid and from this pool
        unsafe {
            pool.remove(handle2.into_shared());
        }
        
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        
        // Pool should be functional with MayDropContents policy
        // (When pool is dropped with items, it should clean up properly)
    }

    #[test]
    fn builder_with_must_not_drop_contents_policy() {
        let mut pool = RawBlindPoolBuilder::new()
            .drop_policy(DropPolicy::MustNotDropContents)
            .build();

        // Insert and immediately remove to test the policy works
        let handle = pool.insert(100_i64);
        assert_eq!(pool.len(), 1);
        assert_eq!(*handle, 100);
        
        // SAFETY: Handle is valid and from this pool
        unsafe {
            pool.remove(handle.into_shared());
        }
        assert_eq!(pool.len(), 0);
        
        // With MustNotDropContents, pool should panic if dropped with contents
        // But since we cleaned up, this should be fine
    }
}
