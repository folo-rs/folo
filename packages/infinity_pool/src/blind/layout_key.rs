use std::alloc::Layout;

/// A collection key derived from a `Layout`.
///
/// Used for fast and efficient lookup by transforming a `Layout` into a single integer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct LayoutKey {
    value: u64,
}

impl LayoutKey {
    /// Creates a new `LayoutKey` from the layout of `T`.
    ///
    /// The layout must have a size and alignment that can be represented by a `u32`.
    /// Panics if the size or alignment exceeds `u32::MAX`.
    pub(crate) const fn with_layout_of<T>() -> Self {
        let layout = Layout::new::<T>();
        Self::new(layout)
    }

    /// Creates a new `LayoutKey` from the given `Layout`.
    ///
    /// The `Layout` must have a size and alignment that can be represented by a `u32`.
    /// Panics if the size or alignment exceeds `u32::MAX`.
    pub(crate) const fn new(layout: Layout) -> Self {
        let size = layout.size();
        let align = layout.align();

        assert!(size <= u32::MAX as usize, "Layout size exceeds u32::MAX",);
        assert!(
            align <= u32::MAX as usize,
            "Layout alignment exceeds u32::MAX"
        );

        let value = ((size as u64) << 32) | (align as u64);

        Self { value }
    }
}
