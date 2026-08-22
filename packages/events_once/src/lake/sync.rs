use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::sync::{Arc, Mutex};

use plurality::MultiPool;

#[cfg(debug_assertions)]
use crate::EventRegistry;
use crate::{
    EVENT_COUNT_FITS_IN_USIZE, Event, NEVER_POISONED, PooledReceiver, PooledRef, PooledSender,
    ReceiverCore, SenderCore, initialize_event,
};

/// Rents out thread-safe events of different payloads.
///
/// You can use this if you need to constantly create events with different/unknown payload types.
/// Functionally, it is similar to [`EventPool`][crate::EventPool] but does not require generic
/// type parameters.
///
/// # Examples
///
/// ```
/// use std::fmt::Debug;
///
/// use events_once::EventLake;
///
/// # #[tokio::main]
/// # async fn main() {
/// let lake = EventLake::new();
///
/// deliver_payload("Hello from the lake!", &lake).await;
/// deliver_payload(42, &lake).await;
/// # }
///
/// async fn deliver_payload<T>(payload: T, lake: &EventLake)
/// where
///     T: Send + Debug + 'static,
/// {
///     let (tx, rx) = lake.rent::<T>();
///
///     tx.send(payload);
///     let payload = rx.await.unwrap();
///     println!("Received payload: {payload:?}");
/// }
/// ```
#[derive(Clone, Debug)]
pub struct EventLake {
    core: Arc<Core>,
}

/// Owns the heterogeneous event storage and diagnostics shared by lake handles.
struct Core {
    events: Mutex<MultiPool>,

    #[cfg(debug_assertions)]
    registry: Arc<EventRegistry>,
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

impl EventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(Core {
                events: Mutex::new(MultiPool::new()),
                #[cfg(debug_assertions)]
                registry: Arc::new(EventRegistry::new()),
            }),
        }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    /// See [`PooledReceiver`] for the receiver's callback and reentrancy contract.
    #[must_use]
    pub fn rent<T: Send + 'static>(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        let storage = self
            .core
            .events
            .lock()
            .expect(NEVER_POISONED)
            .alloc_uninit_box::<UnsafeCell<Event<T>>>();
        let event = initialize_event(storage);

        #[cfg(debug_assertions)]
        {
            // SAFETY: The event was just initialized and remains alive until the endpoint with
            // cleanup ownership unregisters it immediately before releasing the plurality slot.
            unsafe {
                self.core.registry.register(event);
            }
        }

        // SAFETY: The event is initialized in an occupied plurality slot. The endpoints and the
        // debug registry create only shared references, and the slot remains occupied until the
        // state machine grants one endpoint sole cleanup ownership.
        let event_ref = unsafe {
            PooledRef::new(
                #[cfg(debug_assertions)]
                Arc::clone(&self.core.registry),
                event,
            )
        };

        let inner_sender = SenderCore::new(event_ref.clone());
        let inner_receiver = ReceiverCore::new(event_ref);

        (
            PooledSender::new(inner_sender),
            PooledReceiver::new(inner_receiver),
        )
    }

    /// Returns `true` if no events have currently been rented from the lake.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.core.events.lock().expect(NEVER_POISONED).is_empty()
    }

    /// Returns the number of events that have currently been rented from the lake.
    #[must_use]
    pub fn len(&self) -> usize {
        let len = self.core.events.lock().expect(NEVER_POISONED).len();

        usize::try_from(len).expect(EVENT_COUNT_FITS_IN_USIZE)
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
    /// The diagnostic registry lock is released before this returns, so the caller may pass the
    /// snapshots to user-supplied code without holding any lock. Each snapshot is a shared owner
    /// of the backtrace, so it stays valid even if its event is released in the meantime.
    #[cfg(debug_assertions)]
    fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        self.core.registry.awaiter_backtraces()
    }
}

impl Default for EventLake {
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

    /// Compatible event representations must share the same internal layout pool.
    const EXPECTED_COMPATIBLE_LAYOUT_COUNT: usize = 1;

    assert_impl_all!(EventLake: Clone, Send, Sync);

    assert_impl_all!(
        EventLake: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn concurrent_same_type_rentals_are_usable_across_threads() {
        // The smallest group that guarantees competing allocation access.
        const WORKER_COUNT: usize = 2;

        with_watchdog(|| {
            let lake = EventLake::new();
            let barrier = Barrier::new(WORKER_COUNT);

            thread::scope(|scope| {
                for _ in 0..WORKER_COUNT {
                    let lake = lake.clone();
                    let barrier = &barrier;

                    scope.spawn(move || {
                        barrier.wait();

                        let (sender, receiver) = lake.rent::<i32>();
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
    fn concurrent_same_layout_distinct_type_rentals_are_usable_across_threads() {
        with_watchdog(|| {
            assert_eq!(size_of::<Event<i32>>(), size_of::<Event<u32>>());
            assert_eq!(align_of::<Event<i32>>(), align_of::<Event<u32>>());

            let lake = EventLake::new();
            let barrier = Barrier::new(2);

            thread::scope(|scope| {
                scope.spawn({
                    let lake = lake.clone();
                    let barrier = &barrier;

                    move || {
                        barrier.wait();

                        let (sender, receiver) = lake.rent::<i32>();
                        thread::scope(|scope| {
                            scope.spawn(move || sender.send(42)).join().unwrap();
                        });

                        assert_eq!(receiver.into_value().unwrap(), 42);
                    }
                });

                scope.spawn({
                    let lake = lake.clone();
                    let barrier = &barrier;

                    move || {
                        barrier.wait();

                        let (sender, receiver) = lake.rent::<u32>();
                        thread::scope(|scope| {
                            scope.spawn(move || sender.send(24)).join().unwrap();
                        });

                        assert_eq!(receiver.into_value().unwrap(), 24);
                    }
                });
            });

            assert!(lake.is_empty());
        });
    }

    #[test]
    fn len() {
        let lake = EventLake::new();

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
        let lake = EventLake::new();

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
        assert_eq!(size_of::<Event<i32>>(), size_of::<Event<u32>>());
        assert_eq!(align_of::<Event<i32>>(), align_of::<Event<u32>>());

        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<i32>();

        sender.send(-42);
        assert_eq!(receiver.into_value().unwrap(), -42);
        assert!(lake.is_empty());

        let (sender, receiver) = lake.rent::<u32>();

        sender.send(42);
        assert_eq!(receiver.into_value().unwrap(), 42);
        assert!(lake.is_empty());
        assert_eq!(
            lake.core.events.lock().expect(NEVER_POISONED).layouts(),
            EXPECTED_COMPATIBLE_LAYOUT_COUNT
        );
    }

    #[test]
    fn send_receive_after_lake_dropped() {
        let lake = EventLake::new();

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
        let lake = EventLake::new();

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
        let lake = EventLake::new();
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
        // registry locked.
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
        // which reacquire state from the same thread: inspection takes the diagnostic registry
        // mutex, while renting takes the allocation mutex. `inspect_awaiters()` must release its
        // registry lock before invoking the closure; otherwise this thread would deadlock instead
        // of failing an assertion, so the watchdog bounds the test.
        with_watchdog(|| {
            let lake = EventLake::new();

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
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
        // The closure below drops the endpoints it is being told about, which unregisters their
        // diagnostics from the same thread that is iterating `inspect_awaiters()`. A regression
        // that holds the registry lock across the closure call would deadlock rather than panic,
        // so this test is bounded by a watchdog.
        with_watchdog(|| {
            let lake = EventLake::new();

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
        });
    }
}
