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

#[cfg(test)]
#[allow(
    clippy::undocumented_unsafe_blocks,
    reason = "keep tests concise and easy to read"
)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn raw_opaque_pool_thread_travel() {
        let mut pool =
            unsafe { RawOpaquePoolThreadSafe::new(RawOpaquePool::with_layout_of::<u64>()) };

        let handle1 = pool.insert(123_u64);

        let (mut pool, handle2) = thread::spawn(move || {
            assert_eq!(*unsafe { handle1.as_ref() }, 123_u64);

            unsafe {
                pool.remove(handle1);
            }
            let handle2 = pool.insert(456_u64);
            (pool, handle2)
        })
        .join()
        .unwrap();

        assert_eq!(*unsafe { handle2.as_ref() }, 456_u64);

        pool.insert(789_u64);

        thread::spawn(move || {
            pool.insert(111_u64);

            drop(pool);
        })
        .join()
        .unwrap();
    }
}
