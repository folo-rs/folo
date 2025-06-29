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

mod by_ref;
mod by_arc;
mod by_rc;
mod by_ptr;

pub use by_ref::{ByRefEventReceiver, ByRefEventSender};
pub use by_arc::{ByArcEventReceiver, ByArcEventSender};
pub use by_rc::{ByRcEventReceiver, ByRcEventSender};
pub use by_ptr::{ByPtrEventReceiver, ByPtrEventSender};

/// State of a thread-safe event.
#[derive(Debug)]
enum EventState<T> {
    /// No value has been set yet, and no one is waiting.
    NotSet,
    /// No value has been set yet, but someone is waiting for it.
    Awaiting(Waker),
    /// A value has been set.
    Set(T),
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
/// For single-threaded usage, see [`crate::once::LocalEvent`] which has lower overhead.
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
/// use events::once::Event;
/// use futures::executor::block_on;
///
/// let event = Event::<String>::new();
/// let (sender, receiver) = event.by_ref();
///
/// sender.send("Hello".to_string());
/// let message = block_on(receiver);
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct Event<T>
where
    T: Send,
{
    state: Mutex<EventState<T>>,
    used: AtomicBool,
}

impl<T> Event<T>
where
    T: Send,
{
    /// Creates a new thread-safe event.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(EventState::NotSet),
            used: AtomicBool::new(false),
        }
    }

    /// Returns both the sender and receiver for this event, connected by reference.
    ///
    /// # Panics
    ///
    /// Panics if this method or [`by_ref_checked`](Event::by_ref_checked) has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    /// ```
    pub fn by_ref(&self) -> (ByRefEventSender<'_, T>, ByRefEventReceiver<'_, T>) {
        self.by_ref_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by reference,
    /// or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let endpoints = event.by_ref_checked().unwrap();
    /// let endpoints2 = event.by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_ref_checked(&self) -> Option<(ByRefEventSender<'_, T>, ByRefEventReceiver<'_, T>)> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some((
            ByRefEventSender { event: self },
            ByRefEventReceiver { event: self },
        ))
    }

    /// Returns both the sender and receiver for this event, connected by Arc.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] and provides
    /// endpoints that own an Arc to the event instead of borrowing it.
    ///
    /// # Panics
    ///
    /// Panics if endpoints have already been retrieved via any method.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Arc::new(Event::<i32>::new());
    /// let (sender, receiver) = event.by_arc();
    /// ```
    pub fn by_arc(self: &Arc<Self>) -> (ByArcEventSender<T>, ByArcEventReceiver<T>) {
        self.by_arc_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by Arc,
    /// or [`None`] if endpoints have already been retrieved.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] and provides
    /// endpoints that own an Arc to the event instead of borrowing it.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Arc::new(Event::<i32>::new());
    /// let endpoints = event.by_arc_checked().unwrap();
    /// let endpoints2 = event.by_arc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_arc_checked(
        self: &Arc<Self>,
    ) -> Option<(ByArcEventSender<T>, ByArcEventReceiver<T>)> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some((
            ByArcEventSender {
                event: Arc::clone(self),
            },
            ByArcEventReceiver {
                event: Arc::clone(self),
            },
        ))
    }

    /// Returns both the sender and receiver for this event, connected by Rc.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] and provides
    /// endpoints that own an Rc to the event instead of borrowing it.
    ///
    /// # Panics
    ///
    /// Panics if endpoints have already been retrieved via any method.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Rc::new(Event::<i32>::new());
    /// let (sender, receiver) = event.by_rc();
    /// ```
    pub fn by_rc(self: &Rc<Self>) -> (ByRcEventSender<T>, ByRcEventReceiver<T>) {
        self.by_rc_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by Rc,
    /// or [`None`] if endpoints have already been retrieved.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] and provides
    /// endpoints that own an Rc to the event instead of borrowing it.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Rc::new(Event::<i32>::new());
    /// let endpoints = event.by_rc_checked().unwrap();
    /// let endpoints2 = event.by_rc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_rc_checked(self: &Rc<Self>) -> Option<(ByRcEventSender<T>, ByRcEventReceiver<T>)> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some((
            ByRcEventSender {
                event: Rc::clone(self),
            },
            ByRcEventReceiver {
                event: Rc::clone(self),
            },
        ))
    }

    /// Returns both the sender and receiver for this event, connected by raw pointer.
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
    /// Panics if endpoints have already been retrieved via any method.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::Event;
    ///
    /// let mut event = Event::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.by_ptr() };
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver);
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// ```
    #[must_use]
    pub unsafe fn by_ptr(self: Pin<&mut Self>) -> (ByPtrEventSender<T>, ByPtrEventReceiver<T>) {
        // SAFETY: Caller has guaranteed event lifetime management
        unsafe { self.by_ptr_checked() }.expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by raw pointer,
    /// or [`None`] if endpoints have already been retrieved.
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
    /// use events::once::Event;
    ///
    /// let mut event = Event::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let endpoints = unsafe { pinned_event.by_ptr_checked() }.unwrap();
    /// let pinned_event2 = Pin::new(&mut event);
    /// let endpoints2 = unsafe { pinned_event2.by_ptr_checked() }; // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    pub unsafe fn by_ptr_checked(
        self: Pin<&mut Self>,
    ) -> Option<(ByPtrEventSender<T>, ByPtrEventReceiver<T>)> {
        // SAFETY: We need to access the mutable reference to check/set the used flag
        // The caller guarantees the event remains pinned and valid
        let this = unsafe { self.get_unchecked_mut() };
        if this.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        let event_ptr: *const Self = this;
        Some((
            ByPtrEventSender { event: event_ptr },
            ByPtrEventReceiver { event: event_ptr },
        ))
    }

    /// Attempts to set the value of the event.
    ///
    /// Returns `Ok(())` if the value was set successfully, or `Err(value)` if
    /// the event has already been fired.
    pub(crate) fn try_set(&self, value: T) -> Result<(), T> {
        let mut state = self.state.lock().unwrap();
        match mem::replace(&mut *state, EventState::Consumed) {
            EventState::NotSet => {
                *state = EventState::Set(value);
                Ok(())
            }
            EventState::Awaiting(waker) => {
                *state = EventState::Set(value);
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
    pub(crate) fn poll_recv(&self, waker: &Waker) -> Option<T> {
        let mut state = self.state.lock().unwrap();
        match mem::replace(&mut *state, EventState::Consumed) {
            EventState::Set(value) => Some(value),
            EventState::NotSet | EventState::Awaiting(_) => {
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Consumed => None,
        }
    }
}

impl<T> Default for Event<T>
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

    use super::*;

    #[test]
    fn event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.by_ref();
            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<String>::default();
            let (sender, receiver) = event.by_ref();
            sender.send("test".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn event_by_ref_method_provides_both() {
        with_watchdog(|| {
            let event = Event::<u64>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(123);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, 123);
        });
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_by_ref_panics_on_second_call() {
        let event = Event::<i32>::new();
        let _endpoints = event.by_ref();
        let _endpoints2 = event.by_ref(); // Should panic
    }

    #[test]
    fn event_by_ref_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let endpoints1 = event.by_ref_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn event_send_succeeds_without_receiver() {
        let event = Event::<i32>::new();
        let (sender, _receiver) = event.by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn event_works_in_arc() {
        with_watchdog(|| {
            let event = Arc::new(Event::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Arc".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, "Hello from Arc");
        });
    }

    #[test]
    fn event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(Event::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn event_cross_thread_communication() {
        with_watchdog(|| {
            // For cross-thread usage, we need the Event to live long enough
            // In practice, this would typically be done with Arc<Event>
            static EVENT: std::sync::OnceLock<Event<String>> = std::sync::OnceLock::new();
            let event = EVENT.get_or_init(Event::<String>::new);
            let (sender, receiver) = event.by_ref();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from thread!".to_string());
            });
            let receiver_handle = thread::spawn(move || futures::executor::block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello from thread!");
        });
    }

    #[test]
    fn thread_safe_types() {
        // Event should implement Send and Sync
        assert_impl_all!(Event<i32>: Send, Sync);
        // Sender should implement Send and Sync
        assert_impl_all!(ByRefEventSender<'_, i32>: Send, Sync);
        // Receiver should implement Send but not necessarily Sync (based on oneshot::Receiver)
        assert_impl_all!(ByRefEventReceiver<'_, i32>: Send);
    }

    #[test]
    fn event_receive_async_basic() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Event::<i32>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(42);
            let value = block_on(receiver);
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_receive_async_cross_thread() {
        use futures::executor::block_on;

        with_watchdog(|| {
            static EVENT: std::sync::OnceLock<Event<String>> = std::sync::OnceLock::new();
            let event = EVENT.get_or_init(Event::<String>::new);
            let (sender, receiver) = event.by_ref();

            let sender_handle = thread::spawn(move || {
                // Add a small delay to ensure receiver is waiting
                thread::sleep(Duration::from_millis(10));
                sender.send("Hello async!".to_string());
            });

            let receiver_handle = thread::spawn(move || block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello async!");
        });
    }

    #[test]
    fn event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that async can be used in different scenarios
            let event1 = Event::<i32>::new();
            let (sender1, receiver1) = event1.by_ref();

            let event2 = Event::<i32>::new();
            let (sender2, receiver2) = event2.by_ref();

            sender1.send(1);
            sender2.send(2);

            let value1 = block_on(receiver1);
            let value2 = block_on(receiver2);

            assert_eq!(value1, 1);
            assert_eq!(value2, 2);
        });
    }

    #[test]
    fn event_by_arc_basic() {
        with_watchdog(|| {
            let event = Arc::new(Event::<i32>::new());
            let (sender, receiver) = event.by_arc();

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_arc_checked_returns_none_after_use() {
        let event = Arc::new(Event::<i32>::new());
        let endpoints1 = event.by_arc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_arc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_by_arc_panics_on_second_call() {
        let event = Arc::new(Event::<i32>::new());
        let _endpoints = event.by_arc();
        let _endpoints2 = event.by_arc(); // Should panic
    }

    #[test]
    fn event_by_arc_cross_thread() {
        with_watchdog(|| {
            let event = Arc::new(Event::<String>::new());
            let (sender, receiver) = event.by_arc();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from Arc thread!".to_string());
            });

            let receiver_handle = thread::spawn(move || futures::executor::block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello from Arc thread!");
        });
    }

    #[test]
    fn event_by_arc_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Arc::new(Event::<i32>::new());
            let (sender, receiver) = event.by_arc();

            sender.send(42);
            let value = block_on(receiver);
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_rc_basic() {
        with_watchdog(|| {
            let event = Rc::new(Event::<i32>::new());
            let (sender, receiver) = event.by_rc();

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_rc_checked_returns_none_after_use() {
        let event = Rc::new(Event::<i32>::new());
        let endpoints1 = event.by_rc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_rc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_by_rc_panics_on_second_call() {
        let event = Rc::new(Event::<i32>::new());
        let _endpoints = event.by_rc();
        let _endpoints2 = event.by_rc(); // Should panic
    }

    #[test]
    fn event_by_rc_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Rc::new(Event::<i32>::new());
            let (sender, receiver) = event.by_rc();

            sender.send(42);
            let value = block_on(receiver);
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn arc_rc_types_send_sync() {
        // Arc-based types should implement Send and Sync
        assert_impl_all!(ByArcEventSender<i32>: Send, Sync);
        assert_impl_all!(ByArcEventReceiver<i32>: Send);

        // Rc-based types should not implement Send or Sync (due to Rc)
        assert_not_impl_any!(ByRcEventSender<i32>: Send, Sync);
        assert_not_impl_any!(ByRcEventReceiver<i32>: Send, Sync);
    }

    #[test]
    fn event_by_ptr_basic() {
        with_watchdog(|| {
            let mut event = Event::<String>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let mut event = Event::<String>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let endpoints = unsafe { pinned_event.by_ptr_checked() };
            assert!(endpoints.is_some());

            // Second call should return None
            let pinned_event2 = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the endpoints within this test
            let endpoints2 = unsafe { pinned_event2.by_ptr_checked() };
            assert!(endpoints2.is_none());
        });
    }

    #[test]
    fn event_by_ptr_async() {
        with_watchdog(|| {
            let mut event = Event::<i32>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.by_ptr() };

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value, 42);
        });
    }
}
