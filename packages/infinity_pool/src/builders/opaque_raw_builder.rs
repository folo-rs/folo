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
}
