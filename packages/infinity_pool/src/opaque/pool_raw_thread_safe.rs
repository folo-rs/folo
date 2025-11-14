use std::ops::{Deref, DerefMut};

use crate::RawOpaquePool;

/// A wrapper around `RawOpaquePool` that asserts it is `Send` and `Sync`,
/// using the permissions given by the API documentation of `RawOpaquePool`.
#[derive(Debug)]
pub(crate) struct RawOpaquePoolThreadSafe(RawOpaquePool);

impl RawOpaquePoolThreadSafe {
    /// # Safety
    ///
    /// The caller must guarantee that only `Send` objects are inserted into the pool.
    pub(crate) unsafe fn new(inner: RawOpaquePool) -> Self {
        Self(inner)
    }
}

// SAFETY: RawOpaquePool documentation allows us to treat it as `Send` if every object
// inserted into it is `Send`. We guarantee that via ctor requirements, therefore it is `Send`.
unsafe impl Send for RawOpaquePoolThreadSafe {}

// SAFETY: RawOpaquePool documentation allows us to treat it as `Sync` if every object
// inserted into it is `Send`. We guarantee that via ctor requirements, therefore it is `Send`.
unsafe impl Sync for RawOpaquePoolThreadSafe {}

impl Deref for RawOpaquePoolThreadSafe {
    type Target = RawOpaquePool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RawOpaquePoolThreadSafe {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
