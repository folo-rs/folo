//! Single-threaded one-time events.
//!
//! This module provides single-threaded event types that have lower overhead
//! but cannot be shared across threads.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::task::Waker;

use crate::{Disconnected, ValueKind};

mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_ptr::{ByPtrLocalOnceReceiver, ByPtrLocalOnceSender};
pub use by_rc::{ByRcLocalOnceReceiver, ByRcLocalOnceSender};
pub use by_ref::{ByRefLocalOnceReceiver, ByRefLocalOnceSender};

/// State of a single-threaded event.
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

/// A one-time event that can send and receive a value of type `T` (single-threaded variant).
///
/// This is the single-threaded variant that has lower overhead but cannot be shared across threads.
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
/// Likewise, the sender and receiver can only be used once each.
///
/// For thread-safe usage, see [`crate::once::OnceEvent`] which can be used with `Arc<T>`.
///
/// # Single-Threaded Optimizations
///
/// Without thread safety requirements, this type uses more efficient primitives:
/// - [`std::cell::RefCell`] instead of [`std::sync::Mutex`]
/// - [`std::cell::Cell`] instead of [`std::sync::atomic::AtomicBool`]
/// - No atomic operations required
///
/// # Example
///
/// ```rust
/// use events::LocalOnceEvent;
///
/// let event = LocalOnceEvent::<String>::new();
/// let (sender, receiver) = event.bind_by_ref();
///
/// sender.send("Hello".to_string());
/// let message = futures::executor::block_on(receiver).unwrap();
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct LocalOnceEvent<T> {
    state: RefCell<EventState<T>>,
    used: Cell<bool>,
    _single_threaded: PhantomData<*const ()>,
}

impl<T> LocalOnceEvent<T> {
    /// Creates a new single-threaded event.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RefCell::new(EventState::NotSet),
            used: Cell::new(false),
            _single_threaded: PhantomData,
        }
    }

    /// Returns both the sender and receiver for this event, connected by reference.
    ///
    /// # Panics
    ///
    /// Panics if this method or [`bind_by_ref_checked`](LocalOnceEvent::bind_by_ref_checked) has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// let (sender, receiver) = event.bind_by_ref();
    /// ```
    pub fn bind_by_ref(&self) -> (ByRefLocalOnceSender<'_, T>, ByRefLocalOnceReceiver<'_, T>) {
        self.bind_by_ref_checked()
            .expect("OnceEvent has already been bound")
    }

    /// Binds the event to a sender-receiver pair connected by reference,
    /// or [`None`] if the event has already been bound.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// let endpoints = event.bind_by_ref_checked().unwrap();
    /// let endpoints2 = event.bind_by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn bind_by_ref_checked(
        &self,
    ) -> Option<(ByRefLocalOnceSender<'_, T>, ByRefLocalOnceReceiver<'_, T>)> {
        if self.used.get() {
            return None;
        }
        self.used.set(true);

        Some((
            ByRefLocalOnceSender {
                event: self,
                used: false,
            },
            ByRefLocalOnceReceiver { event: self },
        ))
    }

    /// Returns both the sender and receiver for this event, connected by Rc.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] and provides
    /// endpoints that own an Rc to the event.
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let (sender, receiver) = event.bind_by_rc();
    /// ```
    pub fn bind_by_rc(self: &Rc<Self>) -> (ByRcLocalOnceSender<T>, ByRcLocalOnceReceiver<T>) {
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let endpoints = event.bind_by_rc_checked().unwrap();
    /// let endpoints2 = event.bind_by_rc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn bind_by_rc_checked(
        self: &Rc<Self>,
    ) -> Option<(ByRcLocalOnceSender<T>, ByRcLocalOnceReceiver<T>)> {
        if self.used.get() {
            return None;
        }
        self.used.set(true);

        Some((
            ByRcLocalOnceSender {
                event: Rc::clone(self),
                used: false,
            },
            ByRcLocalOnceReceiver {
                event: Rc::clone(self),
            },
        ))
    }

    /// Attempts to set the value of the event.
    ///
    /// Returns `Ok(())` if the value was set successfully, or `Err(value)` if
    /// the event has already been fired.
    pub(crate) fn try_set(&self, value: T) -> Result<(), T> {
        let mut state = self.state.borrow_mut();
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
        let mut state = self.state.borrow_mut();
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
        let mut state = self.state.borrow_mut();

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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = LocalOnceEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_event.bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver).unwrap();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(
        self: Pin<&mut Self>,
    ) -> (ByPtrLocalOnceSender<T>, ByPtrLocalOnceReceiver<T>) {
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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = LocalOnceEvent::<i32>::new();
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
    ) -> Option<(ByPtrLocalOnceSender<T>, ByPtrLocalOnceReceiver<T>)> {
        // SAFETY: We need to access the mutable reference to check/set the used flag
        // The caller guarantees the event remains pinned and valid
        let this = unsafe { self.get_unchecked_mut() };
        if this.used.get() {
            return None;
        }
        this.used.set(true);

        let event_ptr: *const Self = this;
        Some((
            ByPtrLocalOnceSender {
                event: event_ptr,
                used: false,
            },
            ByPtrLocalOnceReceiver { event: event_ptr },
        ))
    }
}

impl<T> Default for LocalOnceEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use static_assertions::assert_not_impl_any;
    use testing::with_watchdog;

    use crate::Disconnected;

    use super::*;

    #[test]
    fn local_event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.bind_by_ref();
            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<String>::default();
            let (sender, receiver) = event.bind_by_ref();
            sender.send("test".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "test");
        });
    }

    #[test]
    fn local_event_by_ref_method_provides_both() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<u64>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(123);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 123);
        });
    }

    #[test]
    #[should_panic(expected = "OnceEvent has already been bound")]
    fn local_event_by_ref_panics_on_second_call() {
        let event = LocalOnceEvent::<i32>::new();
        let _endpoints = event.bind_by_ref();
        let _endpoints2 = event.bind_by_ref(); // Should panic
    }

    #[test]
    fn local_event_by_ref_checked_returns_none_after_use() {
        let event = LocalOnceEvent::<i32>::new();
        let endpoints1 = event.bind_by_ref_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn local_event_send_succeeds_without_receiver() {
        let event = LocalOnceEvent::<i32>::new();
        let (sender, _receiver) = event.bind_by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn local_event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from Rc");
        });
    }

    #[test]
    fn single_threaded_types() {
        // LocalOnceEvent should not implement Send or Sync due to RefCell and PhantomData<*const ()>
        assert_not_impl_any!(LocalOnceEvent<i32>: Send, Sync);
    }

    #[test]
    fn local_event_receive_async_basic() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = LocalOnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(42);
            let value = block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that async can be used in different scenarios
            let event1 = LocalOnceEvent::<i32>::new();
            let (sender1, receiver1) = event1.bind_by_ref();

            let event2 = LocalOnceEvent::<i32>::new();
            let (sender2, receiver2) = event2.bind_by_ref();

            sender1.send(1);
            sender2.send(2);

            let value1 = block_on(receiver1);
            let value2 = block_on(receiver2);

            assert_eq!(value1.unwrap(), 1);
            assert_eq!(value2.unwrap(), 2);
        });
    }

    #[test]
    fn local_event_receive_async_string() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = LocalOnceEvent::<String>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello async world!".to_string());
            let message = block_on(receiver);
            assert_eq!(message.unwrap(), "Hello async world!");
        });
    }

    #[test]
    fn local_event_by_rc_basic() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_by_rc_checked_returns_none_after_use() {
        let event = Rc::new(LocalOnceEvent::<i32>::new());
        let endpoints1 = event.bind_by_rc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_rc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    #[should_panic(expected = "OnceEvent has already been bound")]
    fn local_event_by_rc_panics_on_second_call() {
        let event = Rc::new(LocalOnceEvent::<i32>::new());
        let _endpoints = event.bind_by_rc();
        let _endpoints2 = event.bind_by_rc(); // Should panic
    }

    #[test]
    fn local_event_by_rc_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send(42);
            let value = block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_by_rc_string() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from Rc");
        });
    }

    #[test]
    fn rc_types_not_send_sync() {
        // Rc-based types should not implement Send or Sync
        assert_not_impl_any!(ByRcLocalOnceSender<i32>: Send, Sync);
        assert_not_impl_any!(ByRcLocalOnceReceiver<i32>: Send, Sync);
    }

    #[test]
    fn local_event_by_ptr_basic() {
        with_watchdog(|| {
            let mut event = LocalOnceEvent::<String>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn local_event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let mut event = LocalOnceEvent::<String>::new();
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
    fn local_event_by_ptr_async() {
        with_watchdog(|| {
            let mut event = LocalOnceEvent::<i32>::new();
            let pinned_event = Pin::new(&mut event);
            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { pinned_event.bind_by_ptr() };

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = LocalOnceEvent::<i32>::new();
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
