use std::collections::BTreeMap;
use std::marker::PhantomData;

use crate::Pooled;

/// A pool of pinned objects of any type - it can contain anything while
/// pooling the memory used for it.
///
/// If you squint your eyes, it almost looks like a memory allocator. The big difference is that
/// it is not a general-purpose allocator, but rather a pool of objects that are pinned in memory,
/// and is not exposed via general-purpose allocation APIs.
#[derive(Debug)]
pub struct BlindPool {}

impl BlindPool {
    pub fn insert<T>(&mut self, value: T) -> Pooled<T> {
        todo!()
    }
}

/// Holds memory for a specific kind of memory allocation.
struct BucketOfMemory {

}