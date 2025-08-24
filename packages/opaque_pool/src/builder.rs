use std::alloc::Layout;
use std::cell::Cell;
use std::marker::PhantomData;

use crate::{DropPolicy, OpaquePool};

/// Builder for creating an instance of [`OpaquePool`].
///
/// [`OpaquePool`] requires the item memory layout to be specified at construction time.
/// Use either `.layout()` to provide a specific layout or `.layout_of::<T>()` to generate
/// a layout based on the provided type.
///
/// The layout is mandatory, whereas other settings are optional.
///
/// # Examples
///
/// Using a specific layout:
///
/// ```
/// use std::alloc::Layout;
///
/// use opaque_pool::OpaquePool;
///
/// let layout = Layout::new::<u32>();
/// let pool = OpaquePool::builder().layout(layout).build();
/// ```
///
/// Using type-based layout:
///
/// ```
/// use opaque_pool::OpaquePool;
///
/// let pool = OpaquePool::builder().layout_of::<u64>().build();
/// ```
///
/// # Thread safety
///
/// The builder is thread-mobile ([`Send`]) and can be safely transferred between threads,
/// allowing pool configuration to happen on different threads than where the pool is used.
/// However, it is not thread-safe ([`Sync`]) as it contains mutable configuration state.
#[derive(Debug)]
#[must_use]
pub struct OpaquePoolBuilder {
    item_layout: Option<Layout>,
    drop_policy: DropPolicy,

    // Prevents Sync while allowing Send - builders are thread-mobile but not thread-safe
    _not_sync: PhantomData<Cell<()>>,
}

impl OpaquePoolBuilder {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            item_layout: None,
            drop_policy: DropPolicy::default(),
            _not_sync: PhantomData,
        }
    }

    /// Sets the memory layout for items stored in the pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u32>();
    /// let pool = OpaquePool::builder().layout(layout).build();
    /// ```
    #[inline]
    pub fn layout(mut self, layout: Layout) -> Self {
        assert!(layout.size() > 0, "OpaquePool must have non-zero item size");
        self.item_layout = Some(layout);
        self
    }

    /// Sets the memory layout for items stored in the pool based on a type.
    ///
    /// This is a convenience method that automatically creates the layout for the given type.
    ///
    /// # Examples
    ///
    /// ```
    /// use opaque_pool::OpaquePool;
    ///
    /// let pool = OpaquePool::builder().layout_of::<u64>().build();
    /// ```
    #[inline]
    pub fn layout_of<T>(mut self) -> Self {
        let layout = Layout::new::<T>();
        assert!(layout.size() > 0, "OpaquePool must have non-zero item size");
        self.item_layout = Some(layout);
        self
    }

    /// Sets the [drop policy][DropPolicy] for the pool. This governs how
    /// to treat remaining items in the pool when the pool is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::{DropPolicy, OpaquePool};
    ///
    /// let layout = Layout::new::<u32>();
    /// let pool = OpaquePool::builder()
    ///     .layout(layout)
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    #[inline]
    pub fn drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// Builds the opaque pool with the specified configuration.
    ///
    /// # Panics
    ///
    /// Panics if no layout has been set using either [`layout`](Self::layout) or
    /// [`layout_of`](Self::layout_of).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u32>();
    /// let pool = OpaquePool::builder().layout(layout).build();
    /// ```
    #[must_use]
    #[inline]
    pub fn build(self) -> OpaquePool {
        let layout = self.item_layout.expect(
            "Layout must be set using .layout() or .layout_of::<T>() before calling .build()",
        );
        OpaquePool::new_inner(layout, self.drop_policy)
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::Layout;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::DropPolicy;

    // Test trait implementations.
    assert_impl_all!(OpaquePoolBuilder: Send, std::fmt::Debug);
    assert_not_impl_any!(OpaquePoolBuilder: Sync);

    #[test]
    fn builder_new_creates_default_state() {
        let builder = OpaquePoolBuilder::new();
        assert!(builder.item_layout.is_none());
        assert_eq!(builder.drop_policy, DropPolicy::default());
    }

    #[test]
    fn layout_sets_layout_correctly() {
        let layout = Layout::new::<u64>();
        let builder = OpaquePoolBuilder::new().layout(layout);
        assert_eq!(builder.item_layout, Some(layout));
    }

    #[test]
    fn layout_of_sets_layout_correctly() {
        let builder = OpaquePoolBuilder::new().layout_of::<String>();
        assert_eq!(builder.item_layout, Some(Layout::new::<String>()));
    }

    #[test]
    #[should_panic]
    fn layout_with_zero_size_panics() {
        let layout = Layout::new::<()>();
        let _pool = OpaquePoolBuilder::new().layout(layout).build();
    }

    #[test]
    #[should_panic]
    fn layout_of_zero_sized_type_panics() {
        let _pool = OpaquePoolBuilder::new().layout_of::<()>().build();
    }

    #[test]
    fn drop_policy_sets_policy_correctly() {
        let builder = OpaquePoolBuilder::new().drop_policy(DropPolicy::MustNotDropItems);
        assert_eq!(builder.drop_policy, DropPolicy::MustNotDropItems);

        let builder = OpaquePoolBuilder::new().drop_policy(DropPolicy::MayDropItems);
        assert_eq!(builder.drop_policy, DropPolicy::MayDropItems);
    }

    #[test]
    fn builder_chaining_works() {
        let layout = Layout::new::<i32>();
        let builder = OpaquePoolBuilder::new()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems);

        assert_eq!(builder.item_layout, Some(layout));
        assert_eq!(builder.drop_policy, DropPolicy::MustNotDropItems);
    }

    #[test]
    fn builder_chaining_with_layout_of_works() {
        let builder = OpaquePoolBuilder::new()
            .layout_of::<Vec<String>>()
            .drop_policy(DropPolicy::MayDropItems);

        assert_eq!(builder.item_layout, Some(Layout::new::<Vec<String>>()));
        assert_eq!(builder.drop_policy, DropPolicy::MayDropItems);
    }

    #[test]
    fn layout_can_be_overridden() {
        let layout1 = Layout::new::<u32>();
        let layout2 = Layout::new::<u64>();

        let builder = OpaquePoolBuilder::new().layout(layout1).layout(layout2);
        assert_eq!(builder.item_layout, Some(layout2));
    }

    #[test]
    fn layout_of_can_be_overridden() {
        let builder = OpaquePoolBuilder::new()
            .layout_of::<u32>()
            .layout_of::<u64>();
        assert_eq!(builder.item_layout, Some(Layout::new::<u64>()));
    }

    #[test]
    fn layout_and_layout_of_can_be_mixed() {
        let manual_layout = Layout::new::<String>();
        let builder = OpaquePoolBuilder::new()
            .layout_of::<u64>()
            .layout(manual_layout);
        assert_eq!(builder.item_layout, Some(manual_layout));

        let builder = OpaquePoolBuilder::new()
            .layout(manual_layout)
            .layout_of::<Vec<i32>>();
        assert_eq!(builder.item_layout, Some(Layout::new::<Vec<i32>>()));
    }

    #[test]
    fn drop_policy_can_be_overridden() {
        let builder = OpaquePoolBuilder::new()
            .drop_policy(DropPolicy::MustNotDropItems)
            .drop_policy(DropPolicy::MayDropItems);
        assert_eq!(builder.drop_policy, DropPolicy::MayDropItems);
    }

    #[test]
    fn build_with_layout_succeeds() {
        let layout = Layout::new::<u32>();
        let pool = OpaquePoolBuilder::new().layout(layout).build();
        assert_eq!(pool.item_layout(), layout);
    }

    #[test]
    fn build_with_layout_of_succeeds() {
        let pool = OpaquePoolBuilder::new().layout_of::<String>().build();
        assert_eq!(pool.item_layout(), Layout::new::<String>());
    }

    #[test]
    fn build_with_custom_drop_policy_succeeds() {
        let layout = Layout::new::<i64>();
        let _pool = OpaquePoolBuilder::new()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // This test verifies that the builder accepts custom drop policies.
        // The actual drop policy behavior is tested in pool tests where we can trigger drops.
    }

    #[test]
    #[should_panic]
    fn build_without_layout_panics() {
        let _pool = OpaquePoolBuilder::new().build();
    }

    #[test]
    #[should_panic]
    fn build_with_only_drop_policy_panics() {
        let _pool = OpaquePoolBuilder::new()
            .drop_policy(DropPolicy::MayDropItems)
            .build();
    }

    #[test]
    fn builder_is_debug() {
        let builder = OpaquePoolBuilder::new().layout_of::<u32>();
        let debug_output = format!("{builder:?}");
        assert!(debug_output.contains("OpaquePoolBuilder"));
    }

    #[test]
    fn builder_with_various_layouts() {
        // Test with different size and alignment requirements.
        let _pool1 = OpaquePoolBuilder::new().layout_of::<u8>().build();
        let _pool2 = OpaquePoolBuilder::new().layout_of::<u64>().build();
        let _pool3 = OpaquePoolBuilder::new().layout_of::<[u8; 1000]>().build();
        let _pool4 = OpaquePoolBuilder::new().layout_of::<Vec<String>>().build();

        // Test with custom layouts.
        let custom_layout1 = Layout::from_size_align(42, 1).expect("valid layout");
        let _pool5 = OpaquePoolBuilder::new().layout(custom_layout1).build();

        let custom_layout2 = Layout::from_size_align(16, 8).expect("valid layout");
        let _pool6 = OpaquePoolBuilder::new().layout(custom_layout2).build();
    }

    #[test]
    fn builder_chain_order_independence() {
        let layout = Layout::new::<u32>();

        // Different ordering should produce equivalent pools.
        let pool1 = OpaquePoolBuilder::new()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        let pool2 = OpaquePoolBuilder::new()
            .drop_policy(DropPolicy::MustNotDropItems)
            .layout(layout)
            .build();

        assert_eq!(pool1.item_layout(), pool2.item_layout());
    }

    #[test]
    fn builder_send_trait() {
        fn assert_send<T: Send>() {}
        assert_send::<OpaquePoolBuilder>();

        // Verify builder can be moved between threads.
        let builder = OpaquePoolBuilder::new().layout_of::<u64>();
        let handle = std::thread::spawn(move || builder.build());
        let _pool = handle.join().expect("thread completed successfully");
    }
}
