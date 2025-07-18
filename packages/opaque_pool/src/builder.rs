use std::alloc::Layout;

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
/// ```
/// use std::alloc::Layout;
///
/// use opaque_pool::{DropPolicy, OpaquePool};
///
/// // Using a specific layout.
/// let layout = Layout::new::<u32>();
/// let pool = OpaquePool::builder().layout(layout).build();
///
/// // Using type-based layout.
/// let pool = OpaquePool::builder().layout_of::<u64>().build();
/// ```
#[derive(Debug)]
#[must_use]
pub struct OpaquePoolBuilder {
    item_layout: Option<Layout>,
    drop_policy: DropPolicy,
}

impl OpaquePoolBuilder {
    pub(crate) fn new() -> Self {
        Self {
            item_layout: None,
            drop_policy: DropPolicy::default(),
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
    pub fn build(self) -> OpaquePool {
        let layout = self.item_layout.expect(
            "Layout must be set using .layout() or .layout_of::<T>() before calling .build()",
        );
        OpaquePool::new_inner(layout, self.drop_policy)
    }
}
