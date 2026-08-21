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
///     // SAFETY: The lake is pinned outside this call, and both endpoints are consumed before
///     // the function returns, so their backing pool remains alive and stationary.
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

        // This exact pointer is reconstructed into an owning `Box` exactly once, in
        // `Drop::drop()` below, and nowhere else.
        let core_ptr = Box::into_raw(Box::new(UnsafeCell::new(core)));

        Self {
            // SAFETY: `Box::into_raw` never returns a null pointer.
            core: unsafe { NonNull::new_unchecked(core_ptr) },
        }
    }

    /// Returns a shared reference to the core.
    fn core(&self) -> &Core {
        // SAFETY: `self.core` is the pointer produced by `Box::into_raw` in `new()`; no method
        // reassigns or moves out of this field, so it remains valid, non-null and properly
        // aligned for `UnsafeCell<Core>` for as long as `self` is alive. Only shared references
        // to the pointee are ever formed (here and via `core_cell.get()` below), never `&mut
        // Core`, so this shared reborrow cannot alias a conflicting exclusive reference.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: The cell holds a live, initialized `Core` per the above. We only ever create
        // shared references to its contents, never `&mut Core`, so no conflicting exclusive
        // reference can exist concurrently.
        unsafe { &*core_cell.get() }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    /// See [`RawPooledReceiver`] for the receiver's callback and reentrancy contract.
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
        // SAFETY: `self.core` is the unchanged pointer returned by `Box::into_raw` in `new()` —
        // no method replaces this field and moving `self` does not move the pointee, so this is
        // the unique place that ever converts it back into an owning `Box`, and it happens
        // exactly once (in `Drop::drop`, which the language guarantees runs at most once).
        // `rent()` requires callers to keep the lake alive until every rented endpoint is gone,
        // so by the time this destructor runs, no endpoint still references the allocation and
        // we have exclusive access to reclaim it.
        drop(unsafe { Box::from_raw(self.core.as_ptr()) });
    }
}

// SAFETY: `new()` gives the lake unique ownership of a stable heap allocation (the boxed
// `Core`); moving the lake to another thread moves only the `NonNull` pointer, never the
// pointee, so ownership transfers cleanly. Everything reachable through `Core` is `Send`: the
// map's values are `Pin<Box<dyn ErasedPool>>`, `ErasedPool` requires `Send` (see its trait bound
// below), and the only `ErasedPool` implementation is on `PoolWrapper<T>` for `T: Send`. There is
// therefore nothing non-`Send` reachable from an owned `RawEventLake`.
unsafe impl Send for RawEventLake {}
// SAFETY: Every access to `Core` goes through `core()`, which only ever produces a shared
// `&Core`; `Core` itself has no other accessor. All shared access to the map inside `Core` (in
// `rent()`, `is_empty()`, `len()`, and `awaiter_backtraces()`) is mediated by the `Mutex` around
// it, and no code path ever forms a `&mut Core`. Sharing `&RawEventLake` across threads is
// therefore safe: two threads can only ever contend on the same `Mutex`, never race on
// unsynchronized memory.
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
        // SAFETY: The wrapper is reached only as `Pin<&PoolWrapper<T>>` (see `rent()` on
        // `RawEventLake`, which pins the value once it is stored in the map and never moves it
        // out again). `inner` is a private, directly-owned field that no method ever replaces,
        // swaps or moves out of, so it cannot move independently of the wrapper while the wrapper
        // remains pinned. Projecting to `&inner` is therefore a valid structural pin projection.
        let inner = unsafe { self.map_unchecked(|s| &s.inner) };

        // SAFETY: The top-level `RawEventLake::rent()` caller promises the lake outlives the
        // returned endpoints. The lake owns the pools map and, transitively through this
        // `PoolWrapper`, the `inner` pool for as long as the lake itself lives (the map entry is
        // never removed except when the whole lake is dropped), so that same caller guarantee
        // covers `inner` for the duration required by `RawEventPool::rent()`.
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::task;
    #[cfg(debug_assertions)]
    use std::cell::RefCell;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Barrier;
    use std::task::Waker;
    use std::thread;

    use static_assertions::assert_impl_all;
    #[cfg(debug_assertions)]
    use testing::assert_panics_with;
    use testing::with_watchdog;

    use super::*;
    #[cfg(debug_assertions)]
    use crate::assert_inspect_awaiters_is_reentrant;

    assert_impl_all!(RawEventLake: Send, Sync);

    assert_impl_all!(
        RawEventLake: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn concurrent_same_type_rentals_are_usable_across_threads() {
        // The smallest group that guarantees competing map and pool access.
        const WORKER_COUNT: usize = 2;

        with_watchdog(|| {
            let lake = Box::pin(RawEventLake::new());
            let barrier = Barrier::new(WORKER_COUNT);

            thread::scope(|scope| {
                for _ in 0..WORKER_COUNT {
                    let lake = lake.as_ref().get_ref();
                    let barrier = &barrier;

                    scope.spawn(move || {
                        barrier.wait();

                        // SAFETY: The enclosing scope joins this worker and its sender thread
                        // before the pinned lake is dropped.
                        let (sender, receiver) = unsafe { lake.rent::<i32>() };
                        thread::scope(|scope| {
                            scope.spawn(move || sender.send(42)).join().unwrap();
                        });

                        assert_eq!(receiver.into_value().unwrap(), 42);
                    });
                }
            });

            assert!(lake.is_empty());
        });
    }

    #[test]
    fn concurrent_distinct_type_rentals_are_usable_across_threads() {
        with_watchdog(|| {
            let lake = Box::pin(RawEventLake::new());
            let barrier = Barrier::new(2);

            thread::scope(|scope| {
                scope.spawn({
                    let lake = lake.as_ref().get_ref();
                    let barrier = &barrier;

                    move || {
                        barrier.wait();

                        // SAFETY: The enclosing scope joins this worker and its sender thread
                        // before the pinned lake is dropped.
                        let (sender, receiver) = unsafe { lake.rent::<i32>() };
                        thread::scope(|scope| {
                            scope.spawn(move || sender.send(42)).join().unwrap();
                        });

                        assert_eq!(receiver.into_value().unwrap(), 42);
                    }
                });

                scope.spawn({
                    let lake = lake.as_ref().get_ref();
                    let barrier = &barrier;

                    move || {
                        barrier.wait();

                        // SAFETY: The enclosing scope joins this worker and its sender thread
                        // before the pinned lake is dropped.
                        let (sender, receiver) = unsafe { lake.rent::<String>() };
                        thread::scope(|scope| {
                            scope
                                .spawn(move || sender.send("payload".to_owned()))
                                .join()
                                .unwrap();
                        });

                        assert_eq!(receiver.into_value().unwrap(), "payload");
                    }
                });
            });

            assert!(lake.is_empty());
        });
    }

    #[test]
    fn len() {
        let lake = RawEventLake::new();

        assert_eq!(lake.len(), 0);

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        assert_eq!(lake.len(), 1);

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (sender2, receiver2) = unsafe { lake.rent::<i32>() };
        assert_eq!(lake.len(), 2);

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
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

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
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
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (sender1, receiver1) = unsafe { lake.rent::<String>() };
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (_sender2, receiver2) = unsafe { lake.rent::<i32>() };
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
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
        // The closure below reenters `inspect_awaiters()` and rents from the lake, both of
        // which reacquire the lake's core `Mutex` (and the pool's own mutex) from the same
        // thread. `inspect_awaiters()` must release every lock it holds before invoking the
        // closure; a regression that invokes the closure under a held lock would deadlock this
        // thread forever instead of failing an assertion, so we bound the test with a watchdog.
        with_watchdog(|| {
            let lake = RawEventLake::new();

            // SAFETY: The lake outlives both endpoints.
            let (_sender, receiver) = unsafe { lake.rent::<i32>() };
            let mut receiver = Box::pin(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());
            _ = receiver.as_mut().poll(&mut cx);

            assert_inspect_awaiters_is_reentrant(&|f| lake.inspect_awaiters(f), &|| {
                // A payload type the lake has no pool for yet, to also exercise pool insertion.
                // SAFETY: The lake outlives both endpoints.
                let (sender, receiver) = unsafe { lake.rent::<u8>() };
                drop(sender);
                drop(receiver);
            });
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
        // The closure below drops the endpoints it is being told about, which returns their
        // event to its pool and reacquires the pool's mutex from the same thread that is
        // currently iterating `inspect_awaiters()`. As above, a regression that holds the
        // relevant lock across the closure call would deadlock rather than panic, so this test
        // is bounded by a watchdog.
        with_watchdog(|| {
            let lake = RawEventLake::new();

            let mut cx = task::Context::from_waker(Waker::noop());

            // SAFETY: The lake outlives both endpoints.
            let (sender1, receiver1) = unsafe { lake.rent::<i32>() };
            // SAFETY: The lake outlives both endpoints.
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
        });
    }
}
