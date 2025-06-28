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

use crate::futures::EventFuture;

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
///
/// let event = Event::<String>::new();
/// let (sender, receiver) = event.by_ref();
///
/// sender.send("Hello".to_string());
/// let message = receiver.recv();
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
            ByRefEventSender {
                event: self,
                sent: AtomicBool::new(false),
            },
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
                sent: AtomicBool::new(false),
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
                sent: AtomicBool::new(false),
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
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// ```
    #[must_use]
    pub unsafe fn by_ptr(self: Pin<&mut Self>) -> (ByPtrEventSender<T>, ByPtrEventReceiver<T>) {
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
            ByPtrEventSender {
                event: event_ptr,
                sent: AtomicBool::new(false),
            },
            ByPtrEventReceiver { event: event_ptr },
        ))
    }

    /// Attempts to set the value of the event.
    ///
    /// Returns `Ok(())` if the value was set successfully, or `Err(value)` if
    /// the event has already been fired.
    fn try_set(&self, value: T) -> Result<(), T> {
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

    /// Attempts to receive the value from the event without blocking.
    ///
    /// Returns `Some(value)` if a value is available, or `None` if no value
    /// has been set yet.
    #[allow(
        dead_code,
        reason = "May be useful for non-blocking access in the future"
    )]
    fn try_recv(&self) -> Option<T> {
        let mut state = self.state.lock().unwrap();
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

/// A sender that can send a value through a thread-safe event.
///
/// The sender holds a reference to the event and can be moved across threads.
/// After calling [`send`](ByRefEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefEventSender<'e, T>
where
    T: Send,
{
    event: &'e Event<T>,
    sent: AtomicBool,
}

impl<T> ByRefEventSender<'_, T>
where
    T: Send,
{
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, _receiver) = event.by_ref();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        if self.sent.swap(true, Ordering::SeqCst) {
            return; // Already sent, ignore additional sends
        }
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event.
///
/// The receiver holds a reference to the event and can be moved across threads.
/// After calling [`recv`](ByRefEventReceiver::recv), the receiver is consumed.
#[derive(Debug)]
pub struct ByRefEventReceiver<'e, T>
where
    T: Send,
{
    event: &'e Event<T>,
}

impl<T> ByRefEventReceiver<'_, T>
where
    T: Send,
{
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
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
    /// This method consumes the receiver and returns a future that resolves when a value is sent.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    /// use futures::executor::block_on;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        EventFuture::new(self.event).await
    }
}

/// A sender that can send a value through a thread-safe event using Arc ownership.
///
/// The sender owns an Arc to the event and can be moved across threads.
/// After calling [`send`](ByArcEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByArcEventSender<T>
where
    T: Send,
{
    event: Arc<Event<T>>,
    sent: AtomicBool,
}

impl<T> ByArcEventSender<T>
where
    T: Send,
{
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Arc::new(Event::<i32>::new());
    /// let (sender, _receiver) = event.by_arc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        if self.sent.swap(true, Ordering::SeqCst) {
            return; // Already sent, ignore additional sends
        }
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event using Arc ownership.
///
/// The receiver owns an Arc to the event and can be moved across threads.
/// After calling [`recv`](ByArcEventReceiver::recv), the receiver is consumed.
#[derive(Debug)]
pub struct ByArcEventReceiver<T>
where
    T: Send,
{
    event: Arc<Event<T>>,
}

impl<T> ByArcEventReceiver<T>
where
    T: Send,
{
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
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
    /// This method consumes the receiver and returns a future that resolves when a value is sent.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::once::Event;
    /// use futures::executor::block_on;
    ///
    /// let event = Arc::new(Event::<i32>::new());
    /// let (sender, receiver) = event.by_arc();
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        EventFuture::new(&self.event).await
    }
}

/// A sender that can send a value through a thread-safe event using Rc ownership.
///
/// The sender owns an Rc to the event and is single-threaded.
/// After calling [`send`](ByRcEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRcEventSender<T>
where
    T: Send,
{
    event: Rc<Event<T>>,
    sent: AtomicBool,
}

impl<T> ByRcEventSender<T>
where
    T: Send,
{
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
    /// use events::once::Event;
    ///
    /// let event = Rc::new(Event::<i32>::new());
    /// let (sender, _receiver) = event.by_rc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        if self.sent.swap(true, Ordering::SeqCst) {
            return; // Already sent, ignore additional sends
        }
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event using Rc ownership.
///
/// The receiver owns an Rc to the event and is single-threaded.
/// After calling [`recv`](ByRcEventReceiver::recv), the receiver is consumed.
#[derive(Debug)]
pub struct ByRcEventReceiver<T>
where
    T: Send,
{
    event: Rc<Event<T>>,
}

impl<T> ByRcEventReceiver<T>
where
    T: Send,
{
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
    /// use events::once::Event;
    ///
    /// let event = Rc::new(Event::<i32>::new());
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
    /// This method consumes the receiver and returns a future that resolves when a value is sent.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::Event;
    /// use futures::executor::block_on;
    ///
    /// let event = Rc::new(Event::<i32>::new());
    /// let (sender, receiver) = event.by_rc();
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        EventFuture::new(&self.event).await
    }
}

/// A sender that can send a value through a thread-safe event using raw pointer.
///
/// The sender holds a raw pointer to the event. The caller is responsible for
/// ensuring the event remains valid for the lifetime of the sender.
/// After calling [`send`](ByPtrEventSender::send), the sender is consumed.
///
/// # Safety
///
/// This type is only safe to use when the caller guarantees that the event
/// pointed to remains valid and pinned for the entire lifetime of this sender.
#[derive(Debug)]
pub struct ByPtrEventSender<T>
where
    T: Send,
{
    event: *const Event<T>,
    sent: AtomicBool,
}

// SAFETY: ByPtrEventSender can be Send as long as T: Send, since we only
// send the value T across threads, and the pointer is only used to access
// the thread-safe Event<T>.
unsafe impl<T> Send for ByPtrEventSender<T> where T: Send {}

// SAFETY: ByPtrEventSender can be Sync as long as T: Send, since the
// Event<T> it points to is thread-safe.
unsafe impl<T> Sync for ByPtrEventSender<T> where T: Send {}

impl<T> ByPtrEventSender<T>
where
    T: Send,
{
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
    /// use events::once::Event;
    ///
    /// let mut event = Event::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, _receiver) = unsafe { pinned_event.by_ptr() };
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        if self.sent.swap(true, Ordering::SeqCst) {
            return; // Already sent, ignore additional sends
        }
        // SAFETY: The caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        drop(event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event using raw pointer.
///
/// The receiver holds a raw pointer to the event. The caller is responsible for
/// ensuring the event remains valid for the lifetime of the receiver.
/// After calling [`recv`](ByPtrEventReceiver::recv), the receiver is consumed.
///
/// # Safety
///
/// This type is only safe to use when the caller guarantees that the event
/// pointed to remains valid and pinned for the entire lifetime of this receiver.
#[derive(Debug)]
pub struct ByPtrEventReceiver<T>
where
    T: Send,
{
    event: *const Event<T>,
}

// SAFETY: ByPtrEventReceiver can be Send as long as T: Send, since we only
// receive the value T across threads, and the pointer is only used to access
// the thread-safe Event<T>.
unsafe impl<T> Send for ByPtrEventReceiver<T> where T: Send {}

// Note: We don't implement Sync for ByPtrEventReceiver to match the pattern
// of other receiver types in this codebase.

impl<T> ByPtrEventReceiver<T>
where
    T: Send,
{
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
    /// use events::once::Event;
    ///
    /// let mut event = Event::<i32>::new();
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
    /// This method consumes the receiver and returns a future that resolves when a value is sent.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::Event;
    /// use futures::executor::block_on;
    ///
    /// let mut event = Event::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.by_ptr() };
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(self) -> T {
        // SAFETY: The caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        EventFuture::new(event).await
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
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<String>::default();
            let (sender, receiver) = event.by_ref();
            sender.send("test".to_string());
            let value = receiver.recv();
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn event_by_ref_method_provides_both() {
        with_watchdog(|| {
            let event = Event::<u64>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(123);
            let value = receiver.recv();
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
            let value = receiver.recv();
            assert_eq!(value, "Hello from Arc");
        });
    }

    #[test]
    fn event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(Event::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Rc".to_string());
            let value = receiver.recv();
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

            let receiver_handle = thread::spawn(move || receiver.recv());

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
            let value = block_on(receiver.recv_async());
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

            let receiver_handle = thread::spawn(move || block_on(receiver.recv_async()));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello async!");
        });
    }

    #[test]
    fn event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that sync and async can be used in the same test
            let event1 = Event::<i32>::new();
            let (sender1, receiver1) = event1.by_ref();

            let event2 = Event::<i32>::new();
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
    fn event_try_recv_with_value() {
        with_watchdog(|| {
            let event = Event::<i32>::new();
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
    fn event_try_recv_without_value() {
        with_watchdog(|| {
            let event = Event::<i32>::new();
            let (_sender, _receiver) = event.by_ref();

            // Should return None when no value has been set
            assert_eq!(event.try_recv(), None);
            assert_eq!(event.try_recv(), None); // Multiple calls should still return None
        });
    }

    #[test]
    fn event_try_recv_cross_thread() {
        with_watchdog(|| {
            let event = Arc::new(Event::<String>::new());
            let event_clone = Arc::clone(&event);

            // Set value from another thread
            let handle = thread::spawn(move || {
                let (sender, _receiver) = event_clone.by_ref();
                sender.send("Cross-thread value".to_string());
            });

            handle.join().unwrap();

            // Try to receive from main thread
            assert_eq!(event.try_recv(), Some("Cross-thread value".to_string()));
            assert_eq!(event.try_recv(), None); // Should be consumed
        });
    }

    #[test]
    fn event_by_arc_basic() {
        with_watchdog(|| {
            let event = Arc::new(Event::<i32>::new());
            let (sender, receiver) = event.by_arc();

            sender.send(42);
            let value = receiver.recv();
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

            let receiver_handle = thread::spawn(move || receiver.recv());

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
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_rc_basic() {
        with_watchdog(|| {
            let event = Rc::new(Event::<i32>::new());
            let (sender, receiver) = event.by_rc();

            sender.send(42);
            let value = receiver.recv();
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
            let value = block_on(receiver.recv_async());
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
            let value = receiver.recv();
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
            let value = futures::executor::block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }
}
