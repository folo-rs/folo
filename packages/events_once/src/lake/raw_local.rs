use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;

use plurality::MultiPool;

#[cfg(debug_assertions)]
use crate::LocalEventRegistry;
use crate::{
    EVENT_COUNT_FITS_IN_USIZE, LocalEvent, LocalReceiverCore, LocalSenderCore,
    RawLocalPooledReceiver, RawLocalPooledRef, RawLocalPooledSender, initialize_local_event,
};

/// Rents out single-threaded events of different payloads.
///
/// You can use this if you need to constantly create events with different/unknown payload types.
/// Functionally, it is similar to [`LocalEventPool`][crate::LocalEventPool] but does not require
/// any generic type parameters.
///
/// # Examples
///
/// ```
/// use std::fmt::Debug;
///
/// use events_once::RawLocalEventLake;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let lake = Box::pin(RawLocalEventLake::new());
///
/// deliver_payload("Hello from the lake!", &lake).await;
/// deliver_payload(42, &lake).await;
/// # }
///
/// async fn deliver_payload<T>(payload: T, lake: &RawLocalEventLake)
/// where
///     T: Debug + 'static,
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
pub struct RawLocalEventLake {
    // The boxed core stays at a stable address so debug-only endpoint references can point to its
    // registry. Methods form only shared core references; the multi pool and registry provide the
    // interior mutability needed by their own operations.
    core: NonNull<UnsafeCell<Core>>,
}

/// Owns heterogeneous event storage and diagnostics at a stable address.
struct Core {
    events: MultiPool,

    #[cfg(debug_assertions)]
    registry: LocalEventRegistry,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        f.field("events", &self.events);

        #[cfg(debug_assertions)]
        f.field("registry", &self.registry);

        f.finish()
    }
}

impl RawLocalEventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        let core = Core {
            events: MultiPool::new(),
            #[cfg(debug_assertions)]
            registry: LocalEventRegistry::new(),
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
    /// See [`RawLocalPooledReceiver`] for the receiver's callback and reentrancy contract.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the lake outlives the endpoints.
    #[must_use]
    pub unsafe fn rent<T: 'static>(&self) -> (RawLocalPooledSender<T>, RawLocalPooledReceiver<T>) {
        let core = self.core();
        let storage = core.events.alloc_uninit_box::<UnsafeCell<LocalEvent<T>>>();
        let event = initialize_local_event(storage);

        #[cfg(debug_assertions)]
        {
            // SAFETY: The event was just initialized and remains alive until the endpoint with
            // cleanup ownership unregisters it immediately before releasing the plurality slot.
            unsafe {
                core.registry.register(event);
            }
        }

        // SAFETY: The event is initialized in an occupied plurality slot. Our caller promises the
        // raw lake and its diagnostic registry outlive both endpoints. All access is shared until
        // the state machine grants one endpoint sole cleanup ownership.
        let event_ref = unsafe {
            RawLocalPooledRef::new(
                #[cfg(debug_assertions)]
                NonNull::from(&core.registry),
                event,
            )
        };

        let inner_sender = LocalSenderCore::new(event_ref.clone());
        let inner_receiver = LocalReceiverCore::new(event_ref);

        (
            RawLocalPooledSender::new(inner_sender),
            RawLocalPooledReceiver::new(inner_receiver),
        )
    }

    /// Returns `true` if no events have currently been rented from the lake.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.core().events.is_empty()
    }

    /// Returns the number of events that have currently been rented from the lake.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(self.core().events.len()).expect(EVENT_COUNT_FITS_IN_USIZE)
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
    /// The diagnostic registry borrow is released before this returns, so the caller may pass the
    /// snapshots to user-supplied code without holding any borrow. Each snapshot is a shared owner
    /// of the backtrace, so it stays valid even if its event is released in the meantime.
    #[cfg(debug_assertions)]
    fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        self.core().registry.awaiter_backtraces()
    }
}

impl Default for RawLocalEventLake {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RawLocalEventLake {
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

// The raw core pointer disables auto-trait inference. `MultiPool` and the diagnostic registry
// leave their bookkeeping consistent if allocation or snapshotting unwinds, and payloads are
// reachable only through their endpoints.
impl UnwindSafe for RawLocalEventLake {}
impl RefUnwindSafe for RawLocalEventLake {}
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::task;
    #[cfg(debug_assertions)]
    use std::cell::RefCell;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};
    #[cfg(debug_assertions)]
    use testing::assert_panics_with;

    use super::*;
    #[cfg(debug_assertions)]
    use crate::assert_inspect_awaiters_is_reentrant;

    /// Compatible event representations must share the same internal layout pool.
    const EXPECTED_COMPATIBLE_LAYOUT_COUNT: usize = 1;

    assert_not_impl_any!(RawLocalEventLake: Send, Sync);

    assert_impl_all!(
        RawLocalEventLake: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn len() {
        let lake = RawLocalEventLake::new();

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
        let lake = RawLocalEventLake::new();

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
    fn reuses_compatible_storage_for_distinct_payload_types() {
        assert_eq!(size_of::<LocalEvent<i32>>(), size_of::<LocalEvent<u32>>());
        assert_eq!(align_of::<LocalEvent<i32>>(), align_of::<LocalEvent<u32>>());

        let lake = RawLocalEventLake::new();

        // SAFETY: The lake remains alive until both returned endpoints are consumed.
        let (sender, receiver) = unsafe { lake.rent::<i32>() };
        sender.send(-42);
        assert_eq!(receiver.into_value().unwrap(), -42);
        assert!(lake.is_empty());

        // SAFETY: The lake remains alive until both returned endpoints are consumed.
        let (sender, receiver) = unsafe { lake.rent::<u32>() };
        sender.send(42);
        assert_eq!(receiver.into_value().unwrap(), 42);
        assert!(lake.is_empty());
        assert_eq!(
            lake.core().events.layouts(),
            EXPECTED_COMPATIBLE_LAYOUT_COUNT
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn inspect_awaiters_inspects_awaiters() {
        let lake = RawLocalEventLake::new();

        // 2 events that are awaited and one that is not.
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (sender1, receiver1) = unsafe { lake.rent::<i32>() };
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (_sender2, receiver2) = unsafe { lake.rent::<u32>() };
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (_sender3, _receiver3) = unsafe { lake.rent::<String>() };

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
        let lake = RawLocalEventLake::new();

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
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

        // The lake is still usable, which proves that the panic did not leave the diagnostic
        // registry borrowed.
        let mut call_count = 0;

        lake.inspect_awaiters(|_| {
            call_count += 1;
        });

        assert_eq!(call_count, 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_closure_may_reenter_lake() {
        let lake = RawLocalEventLake::new();

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (_sender, receiver) = unsafe { lake.rent::<i32>() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        assert_inspect_awaiters_is_reentrant(&|f| lake.inspect_awaiters(f), &|| {
            // A new payload layout also exercises heterogeneous slot routing.
            // SAFETY: The lake remains alive until both returned endpoints are dropped.
            let (sender, receiver) = unsafe { lake.rent::<u8>() };
            drop(sender);
            drop(receiver);
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
        let lake = RawLocalEventLake::new();

        let mut cx = task::Context::from_waker(Waker::noop());

        // SAFETY: The lake remains alive until both returned endpoints are dropped.
        let (sender1, receiver1) = unsafe { lake.rent::<i32>() };
        // SAFETY: The lake remains alive until both returned endpoints are dropped.
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
