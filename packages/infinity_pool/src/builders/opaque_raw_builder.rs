use std::alloc::Layout;
use std::marker::PhantomData;

use crate::{DropPolicy, RawOpaquePool};

/// Creates an instance of [`RawOpaquePool`].
#[derive(Debug, Default)]
#[must_use]
pub struct RawOpaquePoolBuilder {
    layout: Option<Layout>,
    drop_policy: DropPolicy,

    _not_send: PhantomData<*const ()>,
}

impl RawOpaquePoolBuilder {
    /// Creates a new instance of the builder with the default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Defines the layout that all objects inserted into the pool must match. Mandatory.
    ///
    /// The layout must have a nonzero size.
    pub fn layout(mut self, layout: Layout) -> Self {
        self.layout = Some(layout);
        self
    }

    /// Defines the layout that all objects inserted into the pool must match. Mandatory.
    ///
    /// The layout must have a nonzero size.
    pub fn layout_of<T: Sized>(mut self) -> Self {
        self.layout = Some(Layout::new::<T>());
        self
    }

    /// Defines the drop policy for the pool. Optional.
    pub fn drop_policy(mut self, drop_policy: DropPolicy) -> Self {
        self.drop_policy = drop_policy;
        self
    }

    /// Validates the options and creates the pool.
    ///
    /// # Panics
    ///
    /// Panics if the layout is not set or if it is a zero-sized layout.
    #[must_use]
    pub fn build(self) -> RawOpaquePool {
        RawOpaquePool::new_inner(self.layout.expect("layout must be set"), self.drop_policy)
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(RawOpaquePoolBuilder: Send, Sync);

    #[test]
    #[should_panic]
    fn layout_not_set_panics() {
        let _pool = RawOpaquePoolBuilder::new().build();
    }

    #[test]
    #[should_panic]
    fn layout_zst_panics() {
        let _pool = RawOpaquePoolBuilder::new()
            .layout(Layout::new::<()>())
            .build();
    }

    #[test]
    fn builder_with_layout_creates_functional_pool() {
        let mut pool = RawOpaquePoolBuilder::new().layout_of::<u64>().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);

        // Test inserting correct type
        let handle = pool.insert(12345_u64);
        assert_eq!(pool.len(), 1);
        assert_eq!(unsafe { *handle.as_ref() }, 12345);

        unsafe {
            pool.remove(handle.into_shared());
        }
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn builder_with_explicit_layout() {
        let layout = Layout::new::<u32>();
        let mut pool = RawOpaquePoolBuilder::new().layout(layout).build();

        // Verify the layout is respected
        let handle = pool.insert(1234);
        assert_eq!(pool.len(), 1);
        assert_eq!(unsafe { *handle.as_ref() }, 1234);

        unsafe {
            pool.remove(handle.into_shared());
        }
    }

    #[test]
    fn builder_with_may_drop_contents_policy() {
        // Test that MayDropContents allows pool to be dropped with items still in it
        let mut pool = RawOpaquePoolBuilder::new()
            .layout_of::<String>()
            .drop_policy(DropPolicy::MayDropContents)
            .build();

        // Insert items and deliberately DO NOT remove them
        let _handle1 = pool.insert("test1".to_string());
        let _handle2 = pool.insert("test2".to_string());

        assert_eq!(pool.len(), 2);

        // Pool should be dropped successfully even with items still in it
        // (This is the whole point of MayDropContents policy)
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn builder_with_must_not_drop_contents_policy() {
        // Test that MustNotDropContents panics when pool is dropped with items
        let mut pool = RawOpaquePoolBuilder::new()
            .layout_of::<i32>()
            .drop_policy(DropPolicy::MustNotDropContents)
            .build();

        // Insert an item and deliberately DO NOT remove it
        let _handle = pool.insert(999_i32);
        assert_eq!(pool.len(), 1);

        // Pool should panic when dropped with items still in it
        // (This is the whole point of MustNotDropContents policy)
        drop(pool);
    }

    #[test]
    fn builder_method_chaining() {
        // Test that method chaining works in any order
        let mut pool1 = RawOpaquePoolBuilder::new()
            .layout_of::<u32>()
            .drop_policy(DropPolicy::MayDropContents)
            .build();

        let mut pool2 = RawOpaquePoolBuilder::new()
            .drop_policy(DropPolicy::MayDropContents)
            .layout_of::<u32>()
            .build();

        // Both should work identically
        let handle1 = pool1.insert(100_u32);
        let handle2 = pool2.insert(200_u32);

        assert_eq!(unsafe { *handle1.as_ref() }, 100);
        assert_eq!(unsafe { *handle2.as_ref() }, 200);
    }
}
