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
#[allow(
    clippy::indexing_slicing,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(RawPinnedPoolBuilder<i32>: Send, Sync);

    #[test]
    fn builder_with_drop_policy() {
        // Test that MayDropContents allows pool to be dropped with items still in it
        let mut pool = RawPinnedPoolBuilder::<String>::new()
            .drop_policy(DropPolicy::MayDropContents)
            .build();

        // Insert an item and deliberately DO NOT remove it
        let _handle = pool.insert("test string".to_string());
        assert_eq!(pool.len(), 1);

        // Pool should be dropped successfully even with items still in it
        // (This is the whole point of MayDropContents policy)
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn builder_with_must_not_drop_contents_policy() {
        // Test that MustNotDropContents panics when pool is dropped with items
        let mut pool = RawPinnedPoolBuilder::<i64>::new()
            .drop_policy(DropPolicy::MustNotDropContents)
            .build();

        // Insert an item and deliberately DO NOT remove it
        let _handle = pool.insert(9999_i64);
        assert_eq!(pool.len(), 1);

        // Pool should panic when dropped with items still in it
        // (This is the whole point of MustNotDropContents policy)
        drop(pool);
    }

    #[test]
    fn builder_default_works() {
        let mut pool = RawPinnedPoolBuilder::<u32>::default().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);

        // Verify it's functional
        let handle = pool.insert(123_u32);
        assert_eq!(unsafe { *handle.as_ref() }, 123);
        // SAFETY: Handle is valid and from this pool
        unsafe {
            pool.remove(handle.into_shared());
        }
    }

    #[test]
    fn builder_via_pool_static_method() {
        let mut pool = RawPinnedPool::<u64>::builder().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);

        // Test functionality
        let handle = pool.insert(64);
        assert_eq!(unsafe { *handle.as_ref() }, 64);

        unsafe {
            pool.remove(handle.into_shared());
        }
    }
}
