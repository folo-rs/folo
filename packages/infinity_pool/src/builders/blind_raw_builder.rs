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
}
