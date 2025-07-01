//! Single-threaded one-time events.
//!
//! This module provides single-threaded event types that have lower overhead
//! but cannot be shared across threads.

use std::cell::{Cell, UnsafeCell};
use std::marker::{PhantomData, PhantomPinned};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::Waker;
use std::{mem, task};

use crate::{Disconnected, Sealed, ValueKind};

/// State of a single-threaded event.
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

/// A one-time event that can send and receive a value of type `T` on a single thread.
///
/// The event can only be used once - after binding a sender and receiver,
/// subsequent bind calls will panic (or return [`None`] for the checked variants).
///
/// Similarly, sending a value will consume the sender, preventing further use.
/// You need to create a new event instance to create another sender-receiver pair.
///
/// To reuse event resources for many operations and avoid constantly recreating events, use
/// [`LocalOnceEventPool`][crate::LocalOnceEventPool].
///
/// For an event that can send the value across threads, see [`OnceEvent`][crate::OnceEvent].
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
    // We only have a get() and a set() that access the state and we guarantee this happens on the
    // same thread, so there is no point in wasting cycles on borrow counting at runtime with
    // RefCell - there cannot be any concurrent access to this field.
    state: UnsafeCell<EventState<T>>,

    // Our API contract requires that an event can only be bound once, so we have to check this
    // because it is not feasible to create an API that can consume the event when creating the
    // sender-receiver pair (all we have might be a shared reference to the event).
    //
    // We may in the future allow unchecked binding to skip this check as an optimization but
    // for now correctness is most important.
    is_bound: Cell<bool>,

    // Everything to do with this event is single-threaded,
    // even if T is thread-mobile or thread-safe.
    _single_threaded: PhantomData<*const ()>,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
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
            state: UnsafeCell::new(EventState::NotSet),
            is_bound: Cell::new(false),
            _single_threaded: PhantomData,
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// let (sender, receiver) = event.bind_by_ref();
    /// ```
    #[must_use]
    pub fn bind_by_ref(
        &self,
    ) -> (
        LocalOnceSender<T, ByRefLocalEvent<'_, T>>,
        LocalOnceReceiver<T, ByRefLocalEvent<'_, T>>,
    ) {
        self.bind_by_ref_checked()
            .expect("LocalOnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
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
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    pub fn bind_by_ref_checked(
        &self,
    ) -> Option<(
        LocalOnceSender<T, ByRefLocalEvent<'_, T>>,
        LocalOnceReceiver<T, ByRefLocalEvent<'_, T>>,
    )> {
        if self.is_bound.replace(true) {
            return None;
        }

        Some((
            LocalOnceSender::new(ByRefLocalEvent { event: self }),
            LocalOnceReceiver::new(ByRefLocalEvent { event: self }),
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let (sender, receiver) = event.bind_by_rc();
    /// ```
    #[must_use]
    pub fn bind_by_rc(
        self: &Rc<Self>,
    ) -> (
        LocalOnceSender<T, ByRcLocalEvent<T>>,
        LocalOnceReceiver<T, ByRcLocalEvent<T>>,
    ) {
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let endpoints = event.bind_by_rc_checked().unwrap();
    /// let endpoints2 = event.bind_by_rc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    pub fn bind_by_rc_checked(
        self: &Rc<Self>,
    ) -> Option<(
        LocalOnceSender<T, ByRcLocalEvent<T>>,
        LocalOnceReceiver<T, ByRcLocalEvent<T>>,
    )> {
        if self.is_bound.replace(true) {
            return None;
        }

        Some((
            LocalOnceSender::new(ByRcLocalEvent {
                event: Rc::clone(self),
            }),
            LocalOnceReceiver::new(ByRcLocalEvent {
                event: Rc::clone(self),
            }),
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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = Box::pin(LocalOnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver, see below.
    /// let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver).unwrap();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (
        LocalOnceSender<T, ByPtrLocalEvent<T>>,
        LocalOnceReceiver<T, ByPtrLocalEvent<T>>,
    ) {
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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = Box::pin(LocalOnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let endpoints = unsafe { event.as_ref().bind_by_ptr_checked() }.unwrap();
    /// let endpoints2 = unsafe { event.as_ref().bind_by_ptr_checked() }; // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    pub unsafe fn bind_by_ptr_checked(
        self: Pin<&Self>,
    ) -> Option<(
        LocalOnceSender<T, ByPtrLocalEvent<T>>,
        LocalOnceReceiver<T, ByPtrLocalEvent<T>>,
    )> {
        if self.is_bound.replace(true) {
            return None;
        }

        let event_ptr = NonNull::from(self.get_ref());

        Some((
            LocalOnceSender::new(ByPtrLocalEvent { event: event_ptr }),
            LocalOnceReceiver::new(ByPtrLocalEvent { event: event_ptr }),
        ))
    }

    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    pub(crate) fn set(&self, result: T) {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

        match &*state {
            EventState::NotSet => {
                *state = EventState::Set(ValueKind::Real(result));
            }
            EventState::Awaiting(_) => {
                let previous_state =
                    mem::replace(&mut *state, EventState::Set(ValueKind::Real(result)));

                match previous_state {
                    EventState::Awaiting(waker) => waker.wake(),
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

    /// We are intended to be polled via `Future::poll`, so we have an equivalent signature here.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been consumed.
    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

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

    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    fn sender_dropped(&self) {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

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

impl<T> Default for LocalOnceEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Enables a sender or receiver to reference the event that connects them.
///
/// This is a sealed trait and exists for internal use only. You never need to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait LocalEventRef<T>: Deref<Target = LocalOnceEvent<T>> + Sealed {}

/// An event referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Copy, Debug)]
pub struct ByRefLocalEvent<'a, T> {
    event: &'a LocalOnceEvent<T>,
}

impl<T> Sealed for ByRefLocalEvent<'_, T> {}
impl<T> LocalEventRef<T> for ByRefLocalEvent<'_, T> {}
impl<T> Deref for ByRefLocalEvent<'_, T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}
impl<T> Clone for ByRefLocalEvent<'_, T> {
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}

/// An event referenced via `Rc` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Debug)]
pub struct ByRcLocalEvent<T> {
    event: Rc<LocalOnceEvent<T>>,
}

impl<T> Sealed for ByRcLocalEvent<T> {}
impl<T> LocalEventRef<T> for ByRcLocalEvent<T> {}
impl<T> Deref for ByRcLocalEvent<T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}
impl<T> Clone for ByRcLocalEvent<T> {
    fn clone(&self) -> Self {
        Self {
            event: Rc::clone(&self.event),
        }
    }
}

/// An event referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Copy, Debug)]
pub struct ByPtrLocalEvent<T> {
    event: NonNull<LocalOnceEvent<T>>,
}

impl<T> Sealed for ByPtrLocalEvent<T> {}
impl<T> LocalEventRef<T> for ByPtrLocalEvent<T> {}
impl<T> Deref for ByPtrLocalEvent<T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for ByPtrLocalEvent<T> {
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}

/// A sender that can send a value through a single-threaded event using Rc ownership.
///
/// The sender owns an Rc to the event and is single-threaded.
/// After calling [`send`](LocalOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct LocalOnceSender<T, R>
where
    R: LocalEventRef<T>,
{
    event_ref: R,

    _t: PhantomData<T>,
}

impl<T, R> LocalOnceSender<T, R>
where
    R: LocalEventRef<T>,
{
    fn new(event_ref: R) -> Self {
        Self {
            event_ref,
            _t: PhantomData,
        }
    }

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
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let (sender, _receiver) = event.bind_by_rc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        self.event_ref.set(value);
    }
}

impl<T, R> Drop for LocalOnceSender<T, R>
where
    R: LocalEventRef<T>,
{
    fn drop(&mut self) {
        self.event_ref.sender_dropped();
    }
}

/// A receiver that can receive a value from a single-threaded event using Rc ownership.
///
/// The receiver owns an Rc to the event and is single-threaded.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct LocalOnceReceiver<T, R>
where
    R: LocalEventRef<T>,
{
    event_ref: R,

    _t: PhantomData<T>,
}

impl<T, R> LocalOnceReceiver<T, R>
where
    R: LocalEventRef<T>,
{
    fn new(event_ref: R) -> Self {
        Self {
            event_ref,
            _t: PhantomData,
        }
    }
}

impl<T, R> Future for LocalOnceReceiver<T, R>
where
    R: LocalEventRef<T>,
{
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        self.event_ref
            .poll(cx.waker())
            .map_or_else(|| task::Poll::Pending, task::Poll::Ready)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use static_assertions::assert_not_impl_any;
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;

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
    fn local_event_by_ref_works() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<u64>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(123);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 123);
        });
    }

    #[test]
    #[should_panic]
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
    fn single_threaded_type() {
        assert_not_impl_any!(LocalOnceEvent<i32>: Send, Sync);
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
    #[should_panic]
    fn local_event_by_rc_panics_on_second_call() {
        let event = Rc::new(LocalOnceEvent::<i32>::new());
        let _endpoints = event.bind_by_rc();
        let _endpoints2 = event.bind_by_rc(); // Should panic
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
    fn local_event_by_ptr_basic() {
        with_watchdog(|| {
            let event = Box::pin(LocalOnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn local_event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let event = Box::pin(LocalOnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let endpoints = unsafe { event.as_ref().bind_by_ptr_checked() };

            assert!(endpoints.is_some());

            // Second call should return None
            // SAFETY: We ensure the event outlives the endpoints within this test
            let endpoints2 = unsafe { event.as_ref().bind_by_ptr_checked() };
            assert!(endpoints2.is_none());
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

    #[test]
    fn thread_safety() {
        // Nothing is Send or Sync - everything is stuck on one thread.
        assert_not_impl_any!(LocalOnceEvent<u32>: Send, Sync);
        assert_not_impl_any!(LocalOnceSender<u32, ByRefLocalEvent<'static, u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceReceiver<u32, ByRefLocalEvent<'static, u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceSender<u32, ByRcLocalEvent<u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceReceiver<u32, ByRcLocalEvent<u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceSender<u32, ByPtrLocalEvent<u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceReceiver<u32, ByPtrLocalEvent<u32>>: Send, Sync);
    }
}
