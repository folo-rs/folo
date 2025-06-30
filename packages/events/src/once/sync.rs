//! Thread-safe one-time events.
//!
//! This module provides thread-safe event types that can be shared across threads
//! and used for cross-thread communication.

use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;

use crate::{Disconnected, ValueKind};

mod by_arc;
mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_arc::{ByArcOnceReceiver, ByArcOnceSender};
pub use by_ptr::{ByPtrOnceReceiver, ByPtrOnceSender};
pub use by_rc::{ByRcOnceReceiver, ByRcOnceSender};
pub use by_ref::{ByRefOnceReceiver, ByRefOnceSender};

/// State of a thread-safe event.
#[derive(Debug)]
enum EventState<T> {
    /// No value has been set yet, and no one is waiting.
    NotSet,
    /// No value has been set yet, but someone is waiting for it.
    Awaiting(Waker),
    /// A value has been set.
    Set(ValueKind<T>),
    /// The value has been consumed.
    Consumed,
}

/// A one-time event that can send and receive a value of type `T`.
///
/// This is the thread-safe variant that can create sender and receiver endpoints
/// that can be used across threads. The event itself can be shared across threads
/// but is typically used as a factory to create endpoints and then discarded.
///
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
///
/// For single-threaded usage, see [`crate::once::LocalOnceEvent`] which has lower overhead.
///
/// # Thread Safety
///
/// This type requires `T: Send` as values will be sent across thread boundaries.
/// The sender and receiver implement `Send + Sync` when `T: Send`.
/// The Event itself is thread-safe because it can be referenced from other threads
/// via the sender and receiver endpoints (which hold references to the Event).
/// This thread-safety is needed even though the Event is typically used once
/// and then discarded.
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
    used: AtomicBool,
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
            used: AtomicBool::new(false),
        }
    }

    /// Binds the event to a sender-receiver pair connected by reference.
    ///
    /// # Panics
    ///
    /// Panics if this method or [`bind_by_ref_checked`](OnceEvent::bind_by_ref_checked) has been called before.
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

    /// Binds the event to a sender-receiver pair connected by reference,
    /// or [`None`] if the event has already been bound.
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
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some((
            ByRefOnceSender { 
                once_event: self,
                used: false,
            },
            ByRefOnceReceiver { once_event: self },
        ))
    }

    /// Binds the event to a sender-receiver pair connected by Arc.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] and provides
    /// endpoints that own an Arc to the event.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound via any method.
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

    /// Binds the event to a sender-receiver pair connected by Arc,
    /// or [`None`] if the event has already been bound.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] and provides
    /// endpoints that own an Arc to the event.
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
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some((
            ByArcOnceSender {
                once_event: Arc::clone(self),
                used: false,
            },
            ByArcOnceReceiver {
                once_event: Arc::clone(self),
            },
        ))
    }

    /// Binds the event to a sender-receiver pair connected by Rc.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] and provides
    /// endpoints that own an Rc to the event.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound via any method.
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

    /// Binds the event to a sender-receiver pair connected by Rc,
    /// or [`None`] if the event has already been bound.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] and provides
    /// endpoints that own an Rc to the event.
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
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some((
            ByRcOnceSender {
                once_event: Rc::clone(self),
                used: false,
            },
            ByRcOnceReceiver {
                once_event: Rc::clone(self),
            },
        ))
    }

    /// Binds the event to a sender-receiver pair connected by raw pointer.
    ///
    /// This method provides endpoints that hold raw pointers to the event instead
    /// of owning or borrowing it. The event must be pinned and the caller is
    /// responsible for ensuring the sender and receiver are dropped before the event.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event remains valid and pinned for the entire lifetime of the sender and receiver
    /// - The sender and receiver are dropped before the event is dropped
    /// - The event is not moved after calling this method
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound via any method.
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
    /// let value = receiver.await;
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(self: Pin<&mut Self>) -> (ByPtrOnceSender<T>, ByPtrOnceReceiver<T>) {
        // SAFETY: Caller has guaranteed event lifetime management
        unsafe { self.bind_by_ptr_checked() }.expect("OnceEvent has already been bound")
    }

    /// Binds the event to a sender-receiver pair connected by raw pointer,
    /// or [`None`] if the event has already been bound.
    ///
    /// This method provides endpoints that hold raw pointers to the event instead
    /// of owning or borrowing it. The event must be pinned and the caller is
    /// responsible for ensuring the sender and receiver are dropped before the event.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event remains valid and pinned for the entire lifetime of the sender and receiver
    /// - The sender and receiver are dropped before the event is dropped
    /// - The event is not moved after calling this method
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
        if this.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        let event_ptr: *const Self = this;
        Some((
            ByPtrOnceSender {
                once_event: event_ptr,
                used: false,
            },
            ByPtrOnceReceiver {
                once_event: event_ptr,
            },
        ))
    }

    /// Attempts to set the value of the event.
    ///
    /// Returns `Ok(())` if the value was set successfully, or `Err(value)` if
    /// the event has already been fired.
    pub(crate) fn try_set(&self, value: T) -> Result<(), T> {
        let mut state = self
            .state
            .lock()
            .expect("event state lock should never be poisoned");
        match mem::replace(&mut *state, EventState::Consumed) {
            EventState::NotSet => {
                *state = EventState::Set(ValueKind::Real(value));
                Ok(())
            }
            EventState::Awaiting(waker) => {
                *state = EventState::Set(ValueKind::Real(value));
                waker.wake();
                Ok(())
            }
            EventState::Set(_) | EventState::Consumed => {
                *state = EventState::Consumed;
                Err(value)
            }
        }
    }

    /// Polls for the value with a waker for async support.
    pub(crate) fn poll_recv(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        let mut state = self
            .state
            .lock()
            .expect("event state lock should never be poisoned");
        match mem::replace(&mut *state, EventState::Consumed) {
            EventState::Set(ValueKind::Real(value)) => Some(Ok(value)),
            EventState::Set(ValueKind::Disconnected) => Some(Err(Disconnected::new())),
            EventState::NotSet | EventState::Awaiting(_) => {
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Consumed => None,
        }
    }

    /// Marks the sender as dropped, potentially waking any waiting receiver.
    pub(crate) fn sender_dropped(&self) {
        let mut state = self
            .state
            .lock()
            .expect("event state lock should never be poisoned");

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
    use std::time::Duration;

    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::with_watchdog;

    use crate::Disconnected;

    use super::*;

    #[test]
    fn event_new_creates_valid_event() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<i32>::new();
                // Should be able to get endpoints once
                let (sender, receiver) = event.bind_by_ref();
                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);
            });
        });
    }

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
    fn event_by_ref_method_provides_both() {
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
    #[should_panic(expected = "OnceEvent has already been bound")]
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
    fn event_cross_thread_communication() {
        with_watchdog(|| {
            // For cross-thread usage, we need the Event to live long enough
            // In practice, this would typically be done with Arc<Event>
            static EVENT: std::sync::OnceLock<OnceEvent<String>> = std::sync::OnceLock::new();
            let event = EVENT.get_or_init(OnceEvent::<String>::new);
            let (sender, receiver) = event.bind_by_ref();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from thread!".to_string());
            });
            let receiver_handle = thread::spawn(move || futures::executor::block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap().unwrap();
            assert_eq!(message, "Hello from thread!");
        });
    }

    #[test]
    fn thread_safe_types() {
        // Event should implement Send and Sync
        assert_impl_all!(OnceEvent<i32>: Send, Sync);
        // Sender should implement Send and Sync
        assert_impl_all!(ByRefOnceSender<'_, i32>: Send, Sync);
        // Receiver should implement Send but not necessarily Sync (based on oneshot::Receiver)
        assert_impl_all!(ByRefOnceReceiver<'_, i32>: Send);
    }

    #[test]
    fn event_receive_async_basic() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = OnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(42);
            let value = block_on(receiver).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_receive_async_cross_thread() {
        use futures::executor::block_on;

        with_watchdog(|| {
            static EVENT: std::sync::OnceLock<OnceEvent<String>> = std::sync::OnceLock::new();
            let event = EVENT.get_or_init(OnceEvent::<String>::new);
            let (sender, receiver) = event.bind_by_ref();

            let sender_handle = thread::spawn(move || {
                // Add a small delay to ensure receiver is waiting
                thread::sleep(Duration::from_millis(10));
                sender.send("Hello async!".to_string());
            });

            let receiver_handle = thread::spawn(move || block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap().unwrap();
            assert_eq!(message, "Hello async!");
        });
    }

    #[test]
    fn event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that async can be used in different scenarios
            let event1 = OnceEvent::<i32>::new();
            let (sender1, receiver1) = event1.bind_by_ref();

            let event2 = OnceEvent::<i32>::new();
            let (sender2, receiver2) = event2.bind_by_ref();

            sender1.send(1);
            sender2.send(2);

            let value1 = block_on(receiver1).unwrap();
            let value2 = block_on(receiver2).unwrap();

            assert_eq!(value1, 1);
            assert_eq!(value2, 2);
        });
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
    #[should_panic(expected = "OnceEvent has already been bound")]
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
    fn event_by_arc_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_arc();

            sender.send(42);
            let value = block_on(receiver).unwrap();
            assert_eq!(value, 42);
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
    #[should_panic(expected = "OnceEvent has already been bound")]
    fn event_by_rc_panics_on_second_call() {
        let event = Rc::new(OnceEvent::<i32>::new());
        let _endpoints = event.bind_by_rc();
        let _endpoints2 = event.bind_by_rc(); // Should panic
    }

    #[test]
    fn event_by_rc_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Rc::new(OnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send(42);
            let value = block_on(receiver).unwrap();
            assert_eq!(value, 42);
        });
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
            let mut event = OnceEvent::<String>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let mut event = OnceEvent::<String>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let endpoints = unsafe { pinned_event.bind_by_ptr_checked() };
            assert!(endpoints.is_some());

            // Second call should return None
            let pinned_event2 = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the endpoints within this test
            let endpoints2 = unsafe { pinned_event2.bind_by_ptr_checked() };
            assert!(endpoints2.is_none());
        });
    }

    #[test]
    fn event_by_ptr_async() {
        with_watchdog(|| {
            let mut event = OnceEvent::<i32>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.bind_by_ptr() };

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);
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
