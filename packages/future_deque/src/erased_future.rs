use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use plurality::{Box as PoolBox, MultiPool, coerce};

/// Type erasure trait for futures stored in a future deque.
///
/// `Future` cannot be erased directly because it carries its result as an associated type,
/// which leaves nothing for the deque to name its element type by. This trait restates
/// `Future::poll` behind a type parameter so that `dyn ErasedFuture<T>` identifies the
/// output type it produces.
pub(crate) trait ErasedFuture<T> {
    /// Polls the underlying future.
    fn poll_erased(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T>;
}

impl<T, F: Future<Output = T>> ErasedFuture<T> for F {
    fn poll_erased(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        self.poll(cx)
    }
}

/// Owning handle to a type-erased future, as stored in a deque slot.
///
/// Both deque variants store this same type. The handle owns its pool slot, so it stays valid
/// after the thread-local pool that produced it is gone.
pub(crate) type ErasedFutureHandle<T> = PoolBox<dyn ErasedFuture<T>>;

/// Moves `future` into the pool and erases its type.
pub(crate) fn alloc_future<T, F: Future<Output = T> + 'static>(
    pool: &MultiPool,
    future: F,
) -> ErasedFutureHandle<T> {
    let handle = pool.alloc_box(future);

    PoolBox::unsize(handle, coerce!(<T> dyn ErasedFuture<T>))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::{Ready, ready};

    use super::*;

    /// Makes capacity growth observable after the initial allocations fill one chunk.
    const TEST_CHUNK_SIZE: u32 = 2;

    #[test]
    fn released_slot_is_reused_with_live_neighbor() {
        let pool = MultiPool::builder().chunk_size(TEST_CHUNK_SIZE).build();
        let held = alloc_future(&pool, ready(1_u32));
        let released = alloc_future(&pool, ready(2_u32));

        assert_eq!(pool.capacity_of::<Ready<u32>>(), u64::from(TEST_CHUNK_SIZE));

        drop(released);
        let replacement = alloc_future(&pool, ready(3_u32));

        assert_eq!(pool.capacity_of::<Ready<u32>>(), u64::from(TEST_CHUNK_SIZE));
        assert_eq!(pool.len_of::<Ready<u32>>(), 2);

        drop((held, replacement));
        assert!(pool.is_empty());
    }
}
