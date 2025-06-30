//! Thread-safe one-time events.
//!
//! This module provides thread-safe event types that can be shared across threads
//! and used for cross-thread communication.

use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;

use crate::{Disconnected, ERR_POISONED_LOCK, ValueKind};

mod by_arc;
mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_arc::*;
pub use by_ptr::*;
pub use by_rc::*;
pub use by_ref::*;

/// State of a thread-safe event.
#[derive(Debug)]
enum EventState<T> {
    /// No value has been set yet, and no one is waiting.
    NotSet,

    /// No value has been set yet, but someone is waiting for it.
    Awaiting(Waker),

    /// A value has been set but nobody has yet started waiting.
    Set(ValueKind<T>),

    /// The value has been set and consumed.
    Consumed,
}

/// A one-time event that can send and receive a value of type `T`, potentially across threads.
///
/// The event can only be used once - after binding a sender and receiver,
/// subsequent bind calls will panic (or return [`None`] for the checked variants).
///
/// Similarly, sending a value will consume the sender, preventing further use.
/// You need to create a new event instance to create another sender-receiver pair.
///
/// To reuse event resources for many operations and avoid constantly recreating events, use
/// [`OnceEventPool`][crate::OnceEventPool].
///
/// For single-threaded usage, see [`LocalOnceEvent`][crate::LocalOnceEvent] which has
/// lower overhead.
///
/// # Example
///
/// ```rust
/// use events::OnceEvent;
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let event = OnceEvent::<String>::new();
/// let (sender, receiver) = event.bind_by_ref();
///
/// sender.send("Hello".to_string());
/// let message = receiver.await.unwrap();
/// assert_eq!(message, "Hello");
/// # });
/// ```
#[derive(Debug)]
pub struct OnceEvent<T>
where
    T: Send,
{
    state: Mutex<EventState<T>>,

    // Our API contract requires that an event can only be bound once, so we have to check this
    // because it is not feasible to create an API that can consume the event when creating the
    // sender-receiver pair (all we have might be a shared reference to the event).
    //
    // We may in the future allow unchecked binding to skip this check as an optimization but
    // for now correctness is most important.
    is_bound: AtomicBool,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
}

impl<T> OnceEvent<T>
where
    T: Send,
{
    /// Creates a new thread-safe event.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(EventState::NotSet),
            is_bound: AtomicBool::new(false),
            _requires_pinning: PhantomPinned,
        }
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// let (sender, receiver) = event.bind_by_ref();
    /// ```
    pub fn bind_by_ref(&self) -> (ByRefOnceSender<'_, T>, ByRefOnceReceiver<'_, T>) {
        self.bind_by_ref_checked()
            .expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// let endpoints = event.bind_by_ref_checked().unwrap();
    /// let endpoints2 = event.bind_by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn bind_by_ref_checked(
        &self,
    ) -> Option<(ByRefOnceSender<'_, T>, ByRefOnceReceiver<'_, T>)> {
        if self.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        Some((
            ByRefOnceSender { once_event: self },
            ByRefOnceReceiver { once_event: self },
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Arc` to the event.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] when called.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let (sender, receiver) = event.bind_by_arc();
    /// ```
    pub fn bind_by_arc(self: &Arc<Self>) -> (ByArcOnceSender<T>, ByArcOnceReceiver<T>) {
        self.bind_by_arc_checked()
            .expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Arc` to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] when called.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let endpoints = event.bind_by_arc_checked().unwrap();
    /// let endpoints2 = event.bind_by_arc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn bind_by_arc_checked(
        self: &Arc<Self>,
    ) -> Option<(ByArcOnceSender<T>, ByArcOnceReceiver<T>)> {
        if self.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        Some((
            ByArcOnceSender {
                once_event: Arc::clone(self),
            },
            ByArcOnceReceiver {
                once_event: Arc::clone(self),
            },
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Rc` to the event.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] when called.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Rc::new(OnceEvent::<i32>::new());
    /// let (sender, receiver) = event.bind_by_rc();
    /// ```
    pub fn bind_by_rc(self: &Rc<Self>) -> (ByRcOnceSender<T>, ByRcOnceReceiver<T>) {
        self.bind_by_rc_checked()
            .expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Rc` to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] when called.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Rc::new(OnceEvent::<i32>::new());
    /// let endpoints = event.bind_by_rc_checked().unwrap();
    /// let endpoints2 = event.bind_by_rc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn bind_by_rc_checked(self: &Rc<Self>) -> Option<(ByRcOnceSender<T>, ByRcOnceReceiver<T>)> {
        if self.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        Some((
            ByRcOnceSender {
                once_event: Rc::clone(self),
            },
            ByRcOnceReceiver {
                once_event: Rc::clone(self),
            },
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a raw pointer to the event.
    ///
    /// This method requires the event to be pinned when called.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event remains alive and pinned for the entire lifetime of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::OnceEvent;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let mut event = OnceEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = receiver.await.unwrap();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(self: Pin<&mut Self>) -> (ByPtrOnceSender<T>, ByPtrOnceReceiver<T>) {
        // SAFETY: Caller has guaranteed event lifetime management
        unsafe { self.bind_by_ptr_checked() }.expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a raw pointer to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// This method requires the event to be pinned when called.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event remains alive and pinned for the entire lifetime of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::OnceEvent;
    ///
    /// let mut event = OnceEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let endpoints = unsafe { pinned_event.bind_by_ptr_checked() }.unwrap();
    /// let pinned_event2 = Pin::new(&mut event);
    /// let endpoints2 = unsafe { pinned_event2.bind_by_ptr_checked() }; // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr_checked(
        self: Pin<&mut Self>,
    ) -> Option<(ByPtrOnceSender<T>, ByPtrOnceReceiver<T>)> {
        // SAFETY: We need to access the mutable reference to check/set the used flag
        // The caller guarantees the event remains pinned and valid
        let this = unsafe { self.get_unchecked_mut() };

        if this.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        let event_ptr: *const Self = this;
        Some((
            ByPtrOnceSender {
                once_event: event_ptr,
            },
            ByPtrOnceReceiver {
                once_event: event_ptr,
            },
        ))
    }

    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    pub(crate) fn set(&self, result: T) {
        let mut waker: Option<Waker> = None;

        {
            let mut state = self.state.lock().expect(ERR_POISONED_LOCK);

            match &*state {
                EventState::NotSet => {
                    *state = EventState::Set(ValueKind::Real(result));
                }
                EventState::Awaiting(_) => {
                    let previous_state =
                        mem::replace(&mut *state, EventState::Set(ValueKind::Real(result)));

                    match previous_state {
                        EventState::Awaiting(w) => waker = Some(w),
                        _ => unreachable!("we are re-matching an already matched pattern"),
                    }
                }
                EventState::Set(_) => {
                    panic!("result already set");
                }
                EventState::Consumed => {
                    panic!("result already consumed");
                }
            }
        }

        // We perform the wakeup outside the lock to avoid unnecessary contention if the receiver
        // of the result wakes up instantly and we have not released our lock yet.
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    // We are intended to be polled via Future::poll, so we have an equivalent signature here.
    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        let mut state = self.state.lock().expect(ERR_POISONED_LOCK);

        match &*state {
            EventState::NotSet => {
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Awaiting(_) => {
                // This is permitted by the Future API contract, in which case only the waker
                // from the most recent poll should be woken up when the result is available.
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Set(_) => {
                let previous_state = mem::replace(&mut *state, EventState::Consumed);

                match previous_state {
                    EventState::Set(result) => match result {
                        ValueKind::Real(value) => Some(Ok(value)),
                        ValueKind::Disconnected => Some(Err(Disconnected::new())),
                    },
                    _ => unreachable!("we are re-matching an already matched pattern"),
                }
            }
            EventState::Consumed => {
                // We do not want to keep a copy of the result around, so we can only return it once.
                // The futures API contract allows us to panic in this situation.
                panic!("event polled after result was already consumed");
            }
        }
    }

    pub(crate) fn sender_dropped(&self) {
        let mut state = self.state.lock().expect(ERR_POISONED_LOCK);

        match &*state {
            EventState::NotSet => {
                *state = EventState::Set(ValueKind::Disconnected);
            }
            EventState::Awaiting(_) => {
                let previous_state =
                    mem::replace(&mut *state, EventState::Set(ValueKind::Disconnected));

                match previous_state {
                    EventState::Awaiting(waker) => waker.wake(),
                    _ => unreachable!("we are re-matching an already matched pattern"),
                }
            }
            _ => {}
        }
    }
}

impl<T> Default for OnceEvent<T>
where
    T: Send,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;

    #[test]
    fn event_default_creates_valid_event() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<String>::default();
                let (sender, receiver) = event.bind_by_ref();
                sender.send("test".to_string());
                let value = receiver.await.unwrap();
                assert_eq!(value, "test");
            });
        });
    }

    #[test]
    fn event_by_ref_works() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<u64>::new();
                let (sender, receiver) = event.bind_by_ref();

                sender.send(123);
                let value = receiver.await.unwrap();
                assert_eq!(value, 123);
            });
        });
    }

    #[test]
    #[should_panic]
    fn event_by_ref_panics_on_second_call() {
        let event = OnceEvent::<i32>::new();
        let _endpoints = event.bind_by_ref();
        let _endpoints2 = event.bind_by_ref(); // Should panic
    }

    #[test]
    fn event_by_ref_checked_returns_none_after_use() {
        let event = OnceEvent::<i32>::new();
        let endpoints1 = event.bind_by_ref_checked();
        assert!(endpoints1.is_some());
        let endpoints2 = event.bind_by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn event_send_succeeds_without_receiver() {
        let event = OnceEvent::<i32>::new();
        let (sender, _receiver) = event.bind_by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn event_works_in_arc() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello from Arc".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from Arc");
        });
    }

    #[test]
    fn event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(OnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn thread_safe_types() {
        assert_impl_all!(OnceEvent<i32>: Send, Sync);
        assert_impl_all!(ByRefOnceSender<'_, i32>: Send, Sync);
        // TODO: It is interesting that `oneshot` Receiver is not Sync. Why not? Should we also not be?
        assert_impl_all!(ByRefOnceReceiver<'_, i32>: Send, Sync);
    }

    #[test]
    fn event_by_arc_basic() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_arc();

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_arc_checked_returns_none_after_use() {
        let event = Arc::new(OnceEvent::<i32>::new());
        let endpoints1 = event.bind_by_arc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_arc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    #[should_panic]
    fn event_by_arc_panics_on_second_call() {
        let event = Arc::new(OnceEvent::<i32>::new());
        let _endpoints = event.bind_by_arc();
        let _endpoints2 = event.bind_by_arc(); // Should panic
    }

    #[test]
    fn event_by_arc_cross_thread() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_arc();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from Arc thread!".to_string());
            });

            let receiver_handle = thread::spawn(move || futures::executor::block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap().unwrap();
            assert_eq!(message, "Hello from Arc thread!");
        });
    }

    #[test]
    fn event_by_rc_basic() {
        with_watchdog(|| {
            let event = Rc::new(OnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_rc_checked_returns_none_after_use() {
        let event = Rc::new(OnceEvent::<i32>::new());
        let endpoints1 = event.bind_by_rc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_rc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    #[should_panic]
    fn event_by_rc_panics_on_second_call() {
        let event = Rc::new(OnceEvent::<i32>::new());
        let _endpoints = event.bind_by_rc();
        let _endpoints2 = event.bind_by_rc(); // Should panic
    }

    #[test]
    fn arc_rc_types_send_sync() {
        // Arc-based types should implement Send and Sync
        assert_impl_all!(ByArcOnceSender<i32>: Send, Sync);
        assert_impl_all!(ByArcOnceReceiver<i32>: Send);

        // Rc-based types should not implement Send or Sync (due to Rc)
        assert_not_impl_any!(ByRcOnceSender<i32>: Send, Sync);
        assert_not_impl_any!(ByRcOnceReceiver<i32>: Send, Sync);
    }

    #[test]
    fn event_by_ptr_basic() {
        with_watchdog(|| {
            let mut event = Box::pin(OnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { event.as_mut().bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let mut event = Box::pin(OnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let endpoints = unsafe { event.as_mut().bind_by_ptr_checked() };
            assert!(endpoints.is_some());

            // Second call should return None
            // SAFETY: We ensure the event outlives the endpoints within this test
            let endpoints2 = unsafe { event.as_mut().bind_by_ptr_checked() };
            assert!(endpoints2.is_none());
        });
    }

    #[test]
    fn event_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<i32>::new();
                let (sender, receiver) = event.bind_by_ref();

                // Drop the sender without sending anything
                drop(sender);

                // Receiver should get a Disconnected error
                let result = receiver.await;
                assert!(result.is_err());
                assert!(matches!(result, Err(Disconnected { .. })));
            });
        });
    }
}
