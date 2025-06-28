//! Single-threaded one-time events.
//!
//! This module provides single-threaded event types that have lower overhead
//! but cannot be shared across threads.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::task::Waker;

use crate::futures::LocalEventFuture;

/// State of a single-threaded event.
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

/// A one-time event that can send and receive a value of type `T` (single-threaded variant).
///
/// This is the single-threaded variant that has lower overhead but cannot be shared across threads.
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
///
/// For thread-safe usage, see [`crate::once::Event`] which can be used with `Arc<T>`.
///
/// # Example
///
/// ```rust
/// use events::once::LocalEvent;
///
/// let event = LocalEvent::<String>::new();
/// let (sender, receiver) = event.by_ref();
///
/// sender.send("Hello".to_string());
/// let message = receiver.recv();
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct LocalEvent<T> {
    state: RefCell<EventState<T>>,
    used: RefCell<bool>,
    _single_threaded: PhantomData<*const ()>,
}

impl<T> LocalEvent<T> {
    /// Creates a new single-threaded event.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RefCell::new(EventState::NotSet),
            used: RefCell::new(false),
            _single_threaded: PhantomData,
        }
    }

    /// Returns both the sender and receiver for this event, connected by reference.
    ///
    /// # Panics
    ///
    /// Panics if this method or [`by_ref_checked`](LocalEvent::by_ref_checked) has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    /// ```
    pub fn by_ref(&self) -> (ByRefLocalEventSender<'_, T>, ByRefLocalEventReceiver<'_, T>) {
        self.by_ref_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by reference,
    /// or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let endpoints = event.by_ref_checked().unwrap();
    /// let endpoints2 = event.by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_ref_checked(
        &self,
    ) -> Option<(ByRefLocalEventSender<'_, T>, ByRefLocalEventReceiver<'_, T>)> {
        let mut used = self.used.borrow_mut();
        if *used {
            return None;
        }
        *used = true;

        Some((
            ByRefLocalEventSender {
                event: self,
                sent: RefCell::new(false),
            },
            ByRefLocalEventReceiver { event: self },
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
    /// use events::once::LocalEvent;
    ///
    /// let event = Rc::new(LocalEvent::<i32>::new());
    /// let (sender, receiver) = event.by_rc();
    /// ```
    pub fn by_rc(self: &Rc<Self>) -> (ByRcLocalEventSender<T>, ByRcLocalEventReceiver<T>) {
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
    /// use events::once::LocalEvent;
    ///
    /// let event = Rc::new(LocalEvent::<i32>::new());
    /// let endpoints = event.by_rc_checked().unwrap();
    /// let endpoints2 = event.by_rc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_rc_checked(
        self: &Rc<Self>,
    ) -> Option<(ByRcLocalEventSender<T>, ByRcLocalEventReceiver<T>)> {
        let mut used = self.used.borrow_mut();
        if *used {
            return None;
        }
        *used = true;

        Some((
            ByRcLocalEventSender {
                event: Rc::clone(self),
                sent: RefCell::new(false),
            },
            ByRcLocalEventReceiver {
                event: Rc::clone(self),
            },
        ))
    }

    /// Attempts to set the value of the event.
    ///
    /// Returns `Ok(())` if the value was set successfully, or `Err(value)` if
    /// the event has already been fired.
    fn try_set(&self, value: T) -> Result<(), T> {
        let mut state = self.state.borrow_mut();
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

    /// Attempts to receive the value from the event without blocking.
    ///
    /// Returns `Some(value)` if a value is available, or `None` if no value
    /// has been set yet.
    #[allow(
        dead_code,
        reason = "May be useful for non-blocking access in the future"
    )]
    fn try_recv(&self) -> Option<T> {
        let mut state = self.state.borrow_mut();
        match mem::replace(&mut *state, EventState::Consumed) {
            EventState::Set(value) => Some(value),
            other => {
                *state = other;
                None
            }
        }
    }

    /// Polls for the value with a waker for async support.
    pub(crate) fn poll_recv(&self, waker: &Waker) -> Option<T> {
        let mut state = self.state.borrow_mut();
        match mem::replace(&mut *state, EventState::Consumed) {
            EventState::Set(value) => Some(value),
            EventState::NotSet | EventState::Awaiting(_) => {
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Consumed => None,
        }
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
    /// use events::once::LocalEvent;
    ///
    /// let mut event = LocalEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.by_ptr() };
    /// 
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// ```
    #[must_use]
    pub unsafe fn by_ptr(self: Pin<&mut Self>) -> (ByPtrLocalEventSender<T>, ByPtrLocalEventReceiver<T>) {
        // SAFETY: Caller has guaranteed event lifetime management
        unsafe { self.by_ptr_checked() }
            .expect("Event endpoints have already been retrieved")
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
    /// use events::once::LocalEvent;
    ///
    /// let mut event = LocalEvent::<i32>::new();
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
    ) -> Option<(ByPtrLocalEventSender<T>, ByPtrLocalEventReceiver<T>)> {
        // SAFETY: We need to access the mutable reference to check/set the used flag
        // The caller guarantees the event remains pinned and valid
        let this = unsafe { self.get_unchecked_mut() };
        let mut used = this.used.borrow_mut();
        if *used {
            return None;
        }
        *used = true;

        let event_ptr: *const Self = this;
        Some((
            ByPtrLocalEventSender {
                event: event_ptr,
                sent: RefCell::new(false),
            },
            ByPtrLocalEventReceiver { event: event_ptr },
        ))
    }
}

/// A one-time event that can send and receive a value of type `T` (single-threaded variant).
///
/// This is the single-threaded variant that has lower overhead but cannot be shared across threads.
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
///
/// For thread-safe usage, see [`crate::once::Event`] which can be used with `Arc<T>`.
///
/// # Example
///
/// ```rust
/// use events::once::LocalEvent;
///
/// let event = LocalEvent::<String>::new();
/// let (sender, receiver) = event.by_ref();
impl<T> Default for LocalEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A sender that can send a value through a single-threaded event.
///
/// The sender holds a reference to the event and can only be used once.
/// After calling [`send`](ByRefLocalEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefLocalEventSender<'e, T> {
    event: &'e LocalEvent<T>,
    sent: RefCell<bool>,
}

impl<T> ByRefLocalEventSender<'_, T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, _receiver) = event.by_ref();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        let mut sent = self.sent.borrow_mut();
        if *sent {
            return; // Already sent, ignore additional sends
        }
        *sent = true;
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a single-threaded event.
///
/// The receiver holds a reference to the event and can only be used once.
/// After calling [`recv`](ByRefLocalEventReceiver::recv), the receiver is consumed.
#[derive(Debug)]
pub struct ByRefLocalEventReceiver<'e, T> {
    event: &'e LocalEvent<T>,
}

impl<T> ByRefLocalEventReceiver<'_, T> {
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn recv(self) -> T {
        // Use block_on from futures crate for synchronous receive
        futures::executor::block_on(self.recv_async())
    }

    /// Receives a value from the event asynchronously.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    /// use futures::executor::block_on;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        LocalEventFuture::new(self.event).await
    }
}

/// A sender that can send a value through a single-threaded event using Rc ownership.
///
/// The sender owns an Rc to the event and is single-threaded.
/// After calling [`send`](ByRcLocalEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRcLocalEventSender<T> {
    event: Rc<LocalEvent<T>>,
    sent: RefCell<bool>,
}

impl<T> ByRcLocalEventSender<T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::LocalEvent;
    ///
    /// let event = Rc::new(LocalEvent::<i32>::new());
    /// let (sender, _receiver) = event.by_rc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        let mut sent = self.sent.borrow_mut();
        if *sent {
            return; // Already sent, ignore additional sends
        }
        *sent = true;
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a single-threaded event using Rc ownership.
///
/// The receiver owns an Rc to the event and is single-threaded.
/// After calling [`recv`](ByRcLocalEventReceiver::recv), the receiver is consumed.
#[derive(Debug)]
pub struct ByRcLocalEventReceiver<T> {
    event: Rc<LocalEvent<T>>,
}

impl<T> ByRcLocalEventReceiver<T> {
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::LocalEvent;
    ///
    /// let event = Rc::new(LocalEvent::<i32>::new());
    /// let (sender, receiver) = event.by_rc();
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn recv(self) -> T {
        // Use block_on from futures crate for synchronous receive
        futures::executor::block_on(self.recv_async())
    }

    /// Receives a value from the event asynchronously.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::LocalEvent;
    /// use futures::executor::block_on;
    ///
    /// let event = Rc::new(LocalEvent::<i32>::new());
    /// let (sender, receiver) = event.by_rc();
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        LocalEventFuture::new(&self.event).await
    }
}

/// A sender that can send a value through a single-threaded event using raw pointer access.
///
/// The sender holds a raw pointer to the event and the caller is responsible for
/// ensuring the event remains valid for the lifetime of the sender.
/// After calling [`send`](ByPtrLocalEventSender::send), the sender is consumed.
///
/// # Safety
///
/// The caller must ensure that the event remains valid and pinned for the entire
/// lifetime of this sender.
#[derive(Debug)]
pub struct ByPtrLocalEventSender<T> {
    event: *const LocalEvent<T>,
    sent: RefCell<bool>,
}

impl<T> ByPtrLocalEventSender<T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::LocalEvent;
    ///
    /// let mut event = LocalEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, _receiver) = unsafe { pinned_event.by_ptr() };
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        let mut sent = self.sent.borrow_mut();
        if *sent {
            return; // Already sent, ignore additional sends
        }
        *sent = true;
        // SAFETY: Caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        drop(event.try_set(value));
    }
}

/// A receiver that can receive a value from a single-threaded event using raw pointer access.
///
/// The receiver holds a raw pointer to the event and the caller is responsible for
/// ensuring the event remains valid for the lifetime of the receiver.
/// After calling [`recv`](ByPtrLocalEventReceiver::recv), the receiver is consumed.
///
/// # Safety
///
/// The caller must ensure that the event remains valid and pinned for the entire
/// lifetime of this receiver.
#[derive(Debug)]
pub struct ByPtrLocalEventReceiver<T> {
    event: *const LocalEvent<T>,
}

impl<T> ByPtrLocalEventReceiver<T> {
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::LocalEvent;
    ///
    /// let mut event = LocalEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.by_ptr() };
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn recv(self) -> T {
        // Use block_on from futures crate for synchronous receive
        futures::executor::block_on(self.recv_async())
    }

    /// Receives a value from the event asynchronously.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::LocalEvent;
    /// use futures::executor::block_on;
    ///
    /// let mut event = LocalEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.by_ptr() };
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        // SAFETY: Caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        LocalEventFuture::new(event).await
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use static_assertions::assert_not_impl_any;
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.by_ref();
            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalEvent::<String>::default();
            let (sender, receiver) = event.by_ref();
            sender.send("test".to_string());
            let value = receiver.recv();
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn local_event_by_ref_method_provides_both() {
        with_watchdog(|| {
            let event = LocalEvent::<u64>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(123);
            let value = receiver.recv();
            assert_eq!(value, 123);
        });
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn local_event_by_ref_panics_on_second_call() {
        let event = LocalEvent::<i32>::new();
        let _endpoints = event.by_ref();
        let _endpoints2 = event.by_ref(); // Should panic
    }

    #[test]
    fn local_event_by_ref_checked_returns_none_after_use() {
        let event = LocalEvent::<i32>::new();
        let endpoints1 = event.by_ref_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn local_event_send_succeeds_without_receiver() {
        let event = LocalEvent::<i32>::new();
        let (sender, _receiver) = event.by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn local_event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(LocalEvent::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Rc".to_string());
            let value = receiver.recv();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn single_threaded_types() {
        // LocalEvent should not implement Send or Sync due to RefCell and PhantomData<*const ()>
        assert_not_impl_any!(LocalEvent<i32>: Send, Sync);
    }

    #[test]
    fn local_event_receive_async_basic() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that sync and async can be used in the same test
            let event1 = LocalEvent::<i32>::new();
            let (sender1, receiver1) = event1.by_ref();

            let event2 = LocalEvent::<i32>::new();
            let (sender2, receiver2) = event2.by_ref();

            sender1.send(1);
            sender2.send(2);

            let value1 = receiver1.recv();
            let value2 = block_on(receiver2.recv_async());

            assert_eq!(value1, 1);
            assert_eq!(value2, 2);
        });
    }

    #[test]
    fn local_event_receive_async_string() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = LocalEvent::<String>::new();
            let (sender, receiver) = event.by_ref();

            sender.send("Hello async world!".to_string());
            let message = block_on(receiver.recv_async());
            assert_eq!(message, "Hello async world!");
        });
    }

    #[test]
    fn local_event_try_recv_with_value() {
        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            let (sender, _receiver) = event.by_ref();

            // Initially should return None
            assert_eq!(event.try_recv(), None);

            // After sending, should return Some(value)
            sender.send(42);
            assert_eq!(event.try_recv(), Some(42));

            // After consuming, should return None again
            assert_eq!(event.try_recv(), None);
        });
    }

    #[test]
    fn local_event_try_recv_without_value() {
        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            let (_sender, _receiver) = event.by_ref();

            // Should return None when no value has been set
            assert_eq!(event.try_recv(), None);
            assert_eq!(event.try_recv(), None); // Multiple calls should still return None
        });
    }

    #[test]
    fn local_event_by_rc_basic() {
        with_watchdog(|| {
            let event = Rc::new(LocalEvent::<i32>::new());
            let (sender, receiver) = event.by_rc();

            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_by_rc_checked_returns_none_after_use() {
        let event = Rc::new(LocalEvent::<i32>::new());
        let endpoints1 = event.by_rc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_rc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn local_event_by_rc_panics_on_second_call() {
        let event = Rc::new(LocalEvent::<i32>::new());
        let _endpoints = event.by_rc();
        let _endpoints2 = event.by_rc(); // Should panic
    }

    #[test]
    fn local_event_by_rc_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Rc::new(LocalEvent::<i32>::new());
            let (sender, receiver) = event.by_rc();

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_by_rc_string() {
        with_watchdog(|| {
            let event = Rc::new(LocalEvent::<String>::new());
            let (sender, receiver) = event.by_rc();

            sender.send("Hello from Rc".to_string());
            let value = receiver.recv();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn rc_types_not_send_sync() {
        // Rc-based types should not implement Send or Sync
        assert_not_impl_any!(ByRcLocalEventSender<i32>: Send, Sync);
        assert_not_impl_any!(ByRcLocalEventReceiver<i32>: Send, Sync);
    }

    #[test]
    fn local_event_by_ptr_basic() {
        with_watchdog(|| {
            let mut event = LocalEvent::<String>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = receiver.recv();
            assert_eq!(value, "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn local_event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let mut event = LocalEvent::<String>::new();
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
    fn local_event_by_ptr_async() {
        with_watchdog(|| {
            let mut event = LocalEvent::<i32>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.by_ptr() };

            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }
}
