use std::alloc::Layout;

use dataless_pool::DatalessPool;
use foldhash::HashMap;

use crate::Pooled;

/// A pool of pinned objects of any type - it can contain anything while
/// pooling the memory used for it.
///
/// If you squint your eyes, it almost looks like a memory allocator. The big difference is that
/// it is not a general-purpose allocator, but rather a pool of objects that are pinned in memory,
/// and is not exposed via general-purpose allocation APIs.
#[derive(Debug)]
pub struct BlindPool {
    /// For each different memory layout we need to support, we use a separate dataless pool.
    ///
    /// The pool is dataless in the sense that it itself knows nothing of the data we put inside,
    /// all it does it give us memory capacity.
    buckets: HashMap<Layout, DatalessPool>,
}

impl BlindPool {
    /// Adds a new item to the pool, returning a [`Pooled<T>`][Pooled] that can be used to access
    /// the item and to later remove it from the pool.
    pub fn insert<T>(&mut self, value: T) -> Pooled<T> {
        todo!()
    }

    /// Removes an item from the pool, given a [`Pooled<T>`][Pooled] that was returned by a
    /// previous call to [`BlindPool::insert()`][Self::insert] on the same pool.
    pub fn remove<T>(&mut self, pooled: Pooled<T>) {
        todo!()
    }
}
