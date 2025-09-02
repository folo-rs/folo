use std::ops::{Deref, DerefMut};

use crate::RawOpaquePool;

/// A wrapper around `RawOpaquePool` that asserts it is `Send`.
#[derive(Debug)]
pub(crate) struct RawOpaquePoolSend(RawOpaquePool);

impl RawOpaquePoolSend {
    /// # Safety
    ///
    /// The caller must guarantee that only `Send` objects are inserted into the pool.
    pub(crate) unsafe fn new(inner: RawOpaquePool) -> Self {
        Self(inner)
    }
}

// SAFETY: RawOpaquePool documentation allows us to treat it as `Send` if every object
// inserted into it is `Send`. We guarantee that via ctor requirements, therefore it is `Send`.
unsafe impl Send for RawOpaquePoolSend {}

impl Deref for RawOpaquePoolSend {
    type Target = RawOpaquePool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RawOpaquePoolSend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
