use std::any::{Any, TypeId, type_name};
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;
use std::sync::Mutex;

use hash_hasher::HashedMap;

use crate::{NEVER_POISONED, RawEventPool, RawPooledReceiver, RawPooledSender};

/// Rents out events of different payloads.
///
/// You can use this if you need to constantly create events with different/unknown payload types.
/// Functionally, it is similar to [`EventPool`][crate::EventPool] but does not require any generic
/// type parameters.
///
/// # Examples
///
/// ```
/// use std::fmt::Debug;
///
/// use events_once::RawEventLake;
///
/// # #[tokio::main]
/// # async fn main() {
/// let lake = Box::pin(RawEventLake::new());
///
/// deliver_payload("Hello from the lake!", &lake).await;
/// deliver_payload(42, &lake).await;
/// # }
///
/// async fn deliver_payload<T>(payload: T, lake: &RawEventLake)
/// where
///     T: Send + Debug + 'static,
/// {
///     // SAFETY: We promise the lake outlives both the returned endpoints.
///     let (tx, rx) = unsafe { lake.rent::<T>() };
///
///     tx.send(payload);
///     let payload = rx.await.unwrap();
///     println!("Received payload: {payload:?}");
/// }
/// ```
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

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
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

    /// Returns a shared reference to the core.
    fn core(&self) -> &Core {
        // SAFETY: We are the owner of the core, so we know it remains valid.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: We only ever create shared references to the core, so no conflicting exclusive
        // references can exist.
        unsafe { &*core_cell.get() }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the lake outlives the endpoints.
    #[must_use]
    pub unsafe fn rent<T: Send + 'static>(&self) -> (RawPooledSender<T>, RawPooledReceiver<T>) {
        let type_id = TypeId::of::<T>();

        let mut pools = self.core().pools.lock().expect(NEVER_POISONED);

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

    /// Returns `true` if no events have currently been rented from the lake.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let pools = self.core().pools.lock().expect(NEVER_POISONED);

        pools.values().all(|x| x.is_empty())
    }

    /// Returns the number of events that have currently been rented from the lake.
    #[must_use]
    pub fn len(&self) -> usize {
        let pools = self.core().pools.lock().expect(NEVER_POISONED);

        pools.values().map(|x| x.len()).sum()
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// awaited event in the lake.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the lake that has been awaited at some point
    /// in the past.
    ///
    /// # Reentrancy
    ///
    /// The closure may freely use this lake: it may rent events, drop endpoints of events it
    /// obtained earlier and call [`inspect_awaiters()`][Self::inspect_awaiters] again. The
    /// backtraces are snapshotted before the first call to the closure, so the sequence of
    /// backtraces the closure receives is unaffected by what the closure does to the lake.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        for backtrace in self.awaiter_backtraces() {
            f(&backtrace);
        }
    }

    /// Snapshots the backtrace of the most recent awaiter of each awaited event in the lake.
    ///
    /// Both the lake lock and the locks of the pools it contains are released before this
    /// returns, so the caller may pass the snapshots to user-supplied code without holding any
    /// lock. Each snapshot is a shared owner of the backtrace, so it stays valid even if its
    /// event is released in the meantime.
    #[cfg(debug_assertions)]
    fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        let pools = self.core().pools.lock().expect(NEVER_POISONED);

        pools
            .values()
            .flat_map(|entry| entry.awaiter_backtraces())
            .collect()
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

// The NonNull<UnsafeCell<Core>> field disables auto-trait inference for
// UnwindSafe/RefUnwindSafe. The pointed-to data is owned by this type and
// protected by a Mutex, so shared references cannot observe inconsistent
// state during unwind.
impl UnwindSafe for RawEventLake {}
impl RefUnwindSafe for RawEventLake {}

struct PoolWrapper<T: 'static> {
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

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
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

    fn is_empty(&self) -> bool;

    fn len(&self) -> usize;

    #[cfg(debug_assertions)]
    fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>>;
}

impl<T: Send + 'static> ErasedPool for PoolWrapper<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(debug_assertions)]
    fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        self.inner.awaiter_backtraces()
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::task;
    #[cfg(debug_assertions)]
    use std::cell::RefCell;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::task::Waker;

    use static_assertions::assert_impl_all;
    #[cfg(debug_assertions)]
    use testing::assert_panics_with;

    use super::*;
    #[cfg(debug_assertions)]
    use crate::assert_inspect_awaiters_is_reentrant;

    assert_impl_all!(RawEventLake: Send, Sync);

    assert_impl_all!(
        RawEventLake: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn len() {
        let lake = RawEventLake::new();

        assert_eq!(lake.len(), 0);

        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        assert_eq!(lake.len(), 1);

        let (sender2, receiver2) = unsafe { lake.rent::<i32>() };
        assert_eq!(lake.len(), 2);

        let (sender3, receiver3) = unsafe { lake.rent::<String>() };
        assert_eq!(lake.len(), 3);

        drop(sender1);
        drop(receiver1);
        assert_eq!(lake.len(), 2);

        drop(sender2);
        drop(receiver2);
        assert_eq!(lake.len(), 1);

        drop(sender3);
        drop(receiver3);
        assert_eq!(lake.len(), 0);
    }

    #[test]
    fn send_receive_multiple_types() {
        let lake = RawEventLake::new();

        assert!(lake.is_empty());

        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        let (sender2, receiver2) = unsafe { lake.rent::<i32>() };

        assert!(!lake.is_empty());

        {
            sender1.send("Hello".to_string());
            sender2.send(42);

            let mut receiver1 = Box::pin(receiver1);
            let mut receiver2 = Box::pin(receiver2);

            let mut cx = task::Context::from_waker(Waker::noop());

            assert_eq!(
                receiver1.as_mut().poll(&mut cx),
                task::Poll::Ready(Ok("Hello".to_string()))
            );
            assert_eq!(receiver2.as_mut().poll(&mut cx), task::Poll::Ready(Ok(42)));
        }

        assert!(lake.is_empty());
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
        let mut receiver2 = Box::pin(receiver2);

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

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_propagates_panic_from_closure() {
        let lake = RawEventLake::new();

        // SAFETY: The lake outlives both endpoints.
        let (_sender, receiver) = unsafe { lake.rent::<i32>() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        assert_panics_with(
            || {
                lake.inspect_awaiters(|_| {
                    panic!("intentional panic to verify pass-through");
                });
            },
            |message| assert!(message.contains("pass-through")),
        );

        // The lake is still usable, which proves that the panic did not leave any lock behind.
        let mut call_count = 0;

        lake.inspect_awaiters(|_| {
            call_count += 1;
        });

        assert_eq!(call_count, 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_closure_may_reenter_lake() {
        let lake = RawEventLake::new();

        let (_sender, receiver) = unsafe { lake.rent::<i32>() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        assert_inspect_awaiters_is_reentrant(&|f| lake.inspect_awaiters(f), &|| {
            // A payload type the lake has no pool for yet, to also exercise pool insertion.
            let (sender, receiver) = unsafe { lake.rent::<u8>() };
            drop(sender);
            drop(receiver);
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
        let lake = RawEventLake::new();

        let mut cx = task::Context::from_waker(Waker::noop());

        let (sender1, receiver1) = unsafe { lake.rent::<i32>() };
        let (sender2, receiver2) = unsafe { lake.rent::<i32>() };

        let mut receiver1 = Box::pin(receiver1);
        let mut receiver2 = Box::pin(receiver2);

        _ = receiver1.as_mut().poll(&mut cx);
        _ = receiver2.as_mut().poll(&mut cx);

        // The closure releases the events it is inspecting. The backtraces it receives are
        // snapshots, so they remain valid and each event is still visited exactly once.
        let endpoints = RefCell::new(vec![(sender1, receiver1), (sender2, receiver2)]);
        let mut call_count = 0;

        lake.inspect_awaiters(|_| {
            call_count += 1;
            drop(endpoints.borrow_mut().pop());
        });

        assert_eq!(call_count, 2);
        assert!(lake.is_empty());
    }
}
