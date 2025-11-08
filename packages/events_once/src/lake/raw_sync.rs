use std::any::{Any, TypeId, type_name};
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use hash_hasher::HashedMap;
use parking_lot::Mutex;

use crate::{RawEventPool, RawPooledReceiver, RawPooledSender};

/// Rents out events of different payloads.
///
/// You can use this if you need to constantly create events with different/unknown payload types.
/// Functionally, it is similar to [`EventPool`] but does not require any generic type parameters.
#[derive(Debug)]
pub struct RawEventLake {
    // This is in an UnsafeCell to logically "detach" it from the parent object.
    // We will create direct (shared) references to the contents of the cell not only from
    // the pool but also from the event references themselves. This is safe as long as
    // we never create conflicting references. We could not guarantee that for the parent
    // object but we can guarantee it for the cell contents.
    core: NonNull<UnsafeCell<Core>>,
}

struct Core {
    // This is a transparent HashMap, meaning it does not do any hashing.
    // The reason is that the TypeId is already a hash, so hashing it again is redundant.
    pools: Mutex<HashedMap<TypeId, Pin<Box<dyn ErasedPool>>>>,
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pools", &self.pools)
            .finish()
    }
}

impl RawEventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        let core = Core {
            pools: Mutex::new(HashedMap::default()),
        };

        let core_ptr = Box::into_raw(Box::new(UnsafeCell::new(core)));

        Self {
            // SAFETY: Boxed object is never null.
            core: unsafe { NonNull::new_unchecked(core_ptr) },
        }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the lake outlives the endpoints.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub unsafe fn rent<T: Send + 'static>(&self) -> (RawPooledSender<T>, RawPooledReceiver<T>) {
        let type_id = TypeId::of::<T>();

        // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let mut pools = core.pools.lock();

        let entry = pools
            .entry(type_id)
            .or_insert_with(|| Box::pin(PoolWrapper::<T>::new()));

        let pool = entry
            .as_any()
            .downcast_ref::<PoolWrapper<T>>()
            .expect("guarded by TypeId");

        // SAFETY: All the pools are pinned, the wrapper just got lost in the downcast.
        let pool = unsafe { Pin::new_unchecked(pool) };

        pool.rent()
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// awaited event in the lake.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the lake that has been awaited at some point
    /// in the past.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let pools = core.pools.lock();

        for entry in pools.values() {
            entry.inspect_awaiters(&mut f);
        }
    }
}

impl Default for RawEventLake {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RawEventLake {
    fn drop(&mut self) {
        // SAFETY: We are the owner of the core, so we know it remains valid.
        // Anyone calling rent() has to promise that we outlive the rented event
        // which means that we must be the last remaining user of the core.
        drop(unsafe { Box::from_raw(self.core.as_ptr()) });
    }
}

// SAFETY: The lake is thread-safe - the only reason it does not have it via auto traits is that
// we have the NonNNull pointer that disables thread safety auto traits. However, all the logic is
// actually protected via the core Mutex, so all is well.
unsafe impl Send for RawEventLake {}
// SAFETY: See above.
unsafe impl Sync for RawEventLake {}

struct PoolWrapper<T: Send + 'static> {
    inner: RawEventPool<T>,
}

impl<T: Send + 'static> PoolWrapper<T> {
    fn new() -> Self {
        Self {
            inner: RawEventPool::new(),
        }
    }

    fn rent(self: Pin<&Self>) -> (RawPooledSender<T>, RawPooledReceiver<T>) {
        // SAFETY: Nothing is being moved here, we are just using the inner pinned value.
        let inner = unsafe { self.map_unchecked(|s| &s.inner) };

        // SAFETY: Forwarding safety guarantees from caller of top-level rent().
        unsafe { inner.rent() }
    }
}

impl<T: Send + 'static> fmt::Debug for PoolWrapper<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

/// A type-erased event pool for which we do not know the payload type any more.
///
/// We downcast from this to a specific pool wrapper when we need to rent events.
trait ErasedPool: fmt::Debug + Send {
    fn as_any(&self) -> &dyn Any;

    #[cfg(debug_assertions)]
    fn inspect_awaiters(&self, f: &mut dyn FnMut(&Backtrace));
}

impl<T: Send + 'static> ErasedPool for PoolWrapper<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    #[cfg(debug_assertions)]
    fn inspect_awaiters(&self, f: &mut dyn FnMut(&Backtrace)) {
        self.inner.inspect_awaiters(|bt| f(bt));
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
mod tests {
    use core::task;
    use std::pin::pin;
    use std::task::Waker;

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(RawEventLake: Send, Sync);

    #[test]
    fn send_receive_multiple_types() {
        let lake = RawEventLake::new();

        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        let (sender2, receiver2) = unsafe { lake.rent::<i32>() };

        sender1.send("Hello".to_string());
        sender2.send(42);

        let receiver1 = pin!(receiver1);
        let receiver2 = pin!(receiver2);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert_eq!(
            receiver1.poll(&mut cx),
            task::Poll::Ready(Ok("Hello".to_string()))
        );
        assert_eq!(receiver2.poll(&mut cx), task::Poll::Ready(Ok(42)));
    }

    #[test]
    #[cfg(debug_assertions)]
    fn inspect_awaiters_inspects_awaiters() {
        let lake = RawEventLake::new();

        // 2 events that are awaited and one that is not.
        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        let (_sender2, receiver2) = unsafe { lake.rent::<i32>() };
        let (_sender3, _receiver3) = unsafe { lake.rent::<f64>() };

        let mut receiver1 = Box::pin(receiver1);
        let mut receiver2 = pin!(receiver2);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert_eq!(receiver1.as_mut().poll(&mut cx), task::Poll::Pending);
        assert_eq!(receiver2.as_mut().poll(&mut cx), task::Poll::Pending);

        let mut call_count = 0;

        lake.inspect_awaiters(|_| {
            call_count += 1;
        });

        assert_eq!(call_count, 2);

        // The first event is dropped, so no longer represented in awaiter inspection.
        drop(sender1);
        drop(receiver1);

        let mut call_count = 0;

        lake.inspect_awaiters(|_| {
            call_count += 1;
        });

        assert_eq!(call_count, 1);
    }
}
