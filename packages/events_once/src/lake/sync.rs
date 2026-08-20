use std::any::{Any, TypeId, type_name};
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::fmt;
use std::sync::{Arc, Mutex};

use hash_hasher::HashedMap;

use crate::{EventPool, NEVER_POISONED, PooledReceiver, PooledSender};

/// Rents out thread-safe events of different payloads.
///
/// You can use this if you need to constantly create events with different/unknown payload types.
/// Functionally, it is similar to [`EventPool`] but does not require any generic type parameters.
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

struct Core {
    // This is a transparent HashMap, meaning it does not do any hashing.
    // The reason is that the TypeId is already a hash, so hashing it again is redundant.
    pools: Mutex<HashedMap<TypeId, Box<dyn ErasedPool>>>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pools", &self.pools)
            .finish()
    }
}

impl EventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(Core {
                pools: Mutex::new(HashedMap::default()),
            }),
        }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    #[must_use]
    pub fn rent<T: Send + 'static>(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        let type_id = TypeId::of::<T>();

        let mut pools = self.core.pools.lock().expect(NEVER_POISONED);

        let entry = pools
            .entry(type_id)
            .or_insert_with(|| Box::new(PoolWrapper::<T>::new()));

        let pool = entry
            .as_any()
            .downcast_ref::<PoolWrapper<T>>()
            .expect("guarded by TypeId");

        pool.rent()
    }

    /// Returns `true` if no events have currently been rented from the lake.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let pools = self.core.pools.lock().expect(NEVER_POISONED);
        pools.values().all(|x| x.is_empty())
    }

    /// Returns the number of events that have currently been rented from the lake.
    #[must_use]
    pub fn len(&self) -> usize {
        let pools = self.core.pools.lock().expect(NEVER_POISONED);
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
        let pools = self.core.pools.lock().expect(NEVER_POISONED);

        pools
            .values()
            .flat_map(|entry| entry.awaiter_backtraces())
            .collect()
    }
}

impl Default for EventLake {
    fn default() -> Self {
        Self::new()
    }
}

struct PoolWrapper<T: 'static> {
    inner: EventPool<T>,
}

impl<T: Send + 'static> PoolWrapper<T> {
    fn new() -> Self {
        Self {
            inner: EventPool::new(),
        }
    }

    fn rent(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        self.inner.rent()
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
    use std::task::Waker;

    use static_assertions::assert_impl_all;
    #[cfg(debug_assertions)]
    use testing::assert_panics_with;
    #[cfg(debug_assertions)]
    use testing::with_watchdog;

    use super::*;
    #[cfg(debug_assertions)]
    use crate::assert_inspect_awaiters_is_reentrant;

    assert_impl_all!(EventLake: Clone, Send, Sync);

    assert_impl_all!(
        EventLake: UnwindSafe, RefUnwindSafe
    );

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
        let (sender1, receiver1) = lake.rent::<String>();
        let (_sender2, receiver2) = lake.rent::<i32>();
        let (_sender3, _receiver3) = lake.rent::<f64>();

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
            let lake = EventLake::new();

            let (_sender, receiver) = lake.rent::<i32>();
            let mut receiver = Box::pin(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());
            _ = receiver.as_mut().poll(&mut cx);

            assert_inspect_awaiters_is_reentrant(&|f| lake.inspect_awaiters(f), &|| {
                // A payload type the lake has no pool for yet, to also exercise pool insertion.
                let (sender, receiver) = lake.rent::<u8>();
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
