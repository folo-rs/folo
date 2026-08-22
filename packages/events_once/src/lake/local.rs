use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
#[cfg(debug_assertions)]
use std::sync::Arc;

use plurality::MultiPool;

#[cfg(debug_assertions)]
use crate::LocalEventRegistry;
use crate::{
    EVENT_COUNT_FITS_IN_USIZE, LocalEvent, LocalReceiverCore, LocalSenderCore, PooledLocalReceiver,
    PooledLocalRef, PooledLocalSender, initialize_local_event,
};

/// Rents out single-threaded events of different payloads.
///
/// You can use this if you need to constantly create single-threaded events with different/unknown
/// payload types. Functionally, it is similar to [`LocalEventPool`][crate::LocalEventPool] but
/// does not require generic type parameters.
///
/// # Examples
///
/// ```
/// use std::fmt::Debug;
///
/// use events_once::LocalEventLake;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let lake = LocalEventLake::new();
///
/// deliver_payload("Hello from the lake!", &lake).await;
/// deliver_payload(42, &lake).await;
/// # }
///
/// async fn deliver_payload<T>(payload: T, lake: &LocalEventLake)
/// where
///     T: Debug + 'static,
/// {
///     let (tx, rx) = lake.rent::<T>();
///
///     tx.send(payload);
///     let payload = rx.await.unwrap();
///     println!("Received payload: {payload:?}");
/// }
/// ```
#[derive(Clone, Debug)]
pub struct LocalEventLake {
    core: Rc<Core>,
}

// `MultiPool` and the diagnostic registry leave their bookkeeping consistent if allocation or
// backtrace snapshotting unwinds. Payloads are reachable only through their endpoints, so a panic
// cannot expose partially initialized event storage through the lake.
impl UnwindSafe for LocalEventLake {}
impl RefUnwindSafe for LocalEventLake {}

/// Owns the heterogeneous event storage and diagnostics shared by lake handles.
struct Core {
    events: MultiPool,

    #[cfg(debug_assertions)]
    registry: Rc<LocalEventRegistry>,
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

impl LocalEventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Rc::new(Core {
                events: MultiPool::new(),
                #[cfg(debug_assertions)]
                registry: Rc::new(LocalEventRegistry::new()),
            }),
        }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    /// See [`PooledLocalReceiver`] for the receiver's callback and reentrancy contract.
    #[must_use]
    pub fn rent<T: 'static>(&self) -> (PooledLocalSender<T>, PooledLocalReceiver<T>) {
        let storage = self
            .core
            .events
            .alloc_uninit_box::<UnsafeCell<LocalEvent<T>>>();
        let event = initialize_local_event(storage);

        #[cfg(debug_assertions)]
        {
            // SAFETY: Plurality keeps this initialized slot at a stable address until release,
            // which occurs only after unregistration. Endpoints and the registry create only shared
            // references to the event throughout that interval.
            unsafe {
                self.core.registry.register(event);
            }
        }

        // SAFETY: The event is initialized in an occupied plurality slot. The endpoints and the
        // debug registry create only shared references, and the slot remains occupied until the
        // state machine grants one endpoint sole cleanup ownership.
        let event_ref = unsafe {
            PooledLocalRef::new(
                #[cfg(debug_assertions)]
                Rc::clone(&self.core.registry),
                event,
            )
        };

        let inner_sender = LocalSenderCore::new(event_ref.clone());
        let inner_receiver = LocalReceiverCore::new(event_ref);

        (
            PooledLocalSender::new(inner_sender),
            PooledLocalReceiver::new(inner_receiver),
        )
    }

    /// Returns `true` if no events have currently been rented from the lake.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.core.events.is_empty()
    }

    /// Returns the number of events that have currently been rented from the lake.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(self.core.events.len()).expect(EVENT_COUNT_FITS_IN_USIZE)
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
        self.core.registry.awaiter_backtraces()
    }
}

impl Default for LocalEventLake {
    fn default() -> Self {
        Self::new()
    }
}

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

    assert_impl_all!(LocalEventLake: Clone);
    assert_not_impl_any!(LocalEventLake: Send, Sync);

    assert_impl_all!(
        LocalEventLake: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn len() {
        let lake = LocalEventLake::new();

        assert_eq!(lake.len(), 0);

        let (sender1, receiver1) = lake.rent::<String>();
        assert_eq!(lake.len(), 1);

        let (sender2, receiver2) = lake.rent::<i32>();
        assert_eq!(lake.len(), 2);

        let (sender3, receiver3) = lake.rent::<String>();
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
        let lake = LocalEventLake::new();

        assert!(lake.is_empty());

        let (sender1, receiver1) = lake.rent::<String>();
        let (sender2, receiver2) = lake.rent::<i32>();

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

        let lake = LocalEventLake::new();
        let (sender, receiver) = lake.rent::<i32>();

        sender.send(-42);
        assert_eq!(receiver.into_value().unwrap(), -42);
        assert!(lake.is_empty());

        let (sender, receiver) = lake.rent::<u32>();

        sender.send(42);
        assert_eq!(receiver.into_value().unwrap(), 42);
        assert!(lake.is_empty());
        assert_eq!(lake.core.events.layouts(), EXPECTED_COMPATIBLE_LAYOUT_COUNT);
    }

    #[test]
    fn send_receive_after_lake_dropped() {
        let lake = LocalEventLake::new();

        let (sender1, receiver1) = lake.rent::<String>();
        let (sender2, receiver2) = lake.rent::<i32>();

        drop(lake);

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

    #[test]
    #[cfg(debug_assertions)]
    fn inspect_awaiters_inspects_awaiters() {
        let lake = LocalEventLake::new();

        // 2 events that are awaited and one that is not.
        let (sender1, receiver1) = lake.rent::<i32>();
        let (_sender2, receiver2) = lake.rent::<u32>();
        let (_sender3, _receiver3) = lake.rent::<String>();

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
        let lake = LocalEventLake::new();
        let (_sender, receiver) = lake.rent::<i32>();
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
        let lake = LocalEventLake::new();

        let (_sender, receiver) = lake.rent::<i32>();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        assert_inspect_awaiters_is_reentrant(&|f| lake.inspect_awaiters(f), &|| {
            // A new payload layout also exercises heterogeneous slot routing.
            let (sender, receiver) = lake.rent::<u8>();
            drop(sender);
            drop(receiver);
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
        let lake = LocalEventLake::new();

        let mut cx = task::Context::from_waker(Waker::noop());

        let (sender1, receiver1) = lake.rent::<i32>();
        let (sender2, receiver2) = lake.rent::<i32>();

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
