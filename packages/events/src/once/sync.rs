//! Thread-safe one-time events.
//!
//! This module provides thread-safe event types that can be shared across threads
//! and used for cross-thread communication.

use std::cell::Cell;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::{mem, task};

use crate::{Disconnected, ERR_POISONED_LOCK, Sealed, ValueKind};

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
/// For single-threaded usage, see [`OnceEvent`][crate::OnceEvent] which has
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
    #[must_use]
    pub fn bind_by_ref(
        &self,
    ) -> (
        OnceSender<T, ByRefEvent<'_, T>>,
        OnceReceiver<T, ByRefEvent<'_, T>>,
    ) {
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
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    #[must_use]
    pub fn bind_by_ref_checked(
        &self,
    ) -> Option<(
        OnceSender<T, ByRefEvent<'_, T>>,
        OnceReceiver<T, ByRefEvent<'_, T>>,
    )> {
        if self.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        Some((
            OnceSender::new(ByRefEvent { event: self }),
            OnceReceiver::new(ByRefEvent { event: self }),
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
    #[must_use]
    pub fn bind_by_arc(
        self: &Arc<Self>,
    ) -> (OnceSender<T, ByArcEvent<T>>, OnceReceiver<T, ByArcEvent<T>>) {
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
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    pub fn bind_by_arc_checked(
        self: &Arc<Self>,
    ) -> Option<(OnceSender<T, ByArcEvent<T>>, OnceReceiver<T, ByArcEvent<T>>)> {
        if self.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        Some((
            OnceSender::new(ByArcEvent {
                event: Arc::clone(self),
            }),
            OnceReceiver::new(ByArcEvent {
                event: Arc::clone(self),
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
    /// use events::OnceEvent;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let mut event = Box::pin(OnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = receiver.await.unwrap();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (OnceSender<T, ByPtrEvent<T>>, OnceReceiver<T, ByPtrEvent<T>>) {
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
    /// let mut event = Box::pin(OnceEvent::<i32>::new());
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
    ) -> Option<(OnceSender<T, ByPtrEvent<T>>, OnceReceiver<T, ByPtrEvent<T>>)> {
        if self.is_bound.swap(true, Ordering::Relaxed) {
            return None;
        }

        let event_ptr = NonNull::from(self.get_ref());

        Some((
            OnceSender::new(ByPtrEvent { event: event_ptr }),
            OnceReceiver::new(ByPtrEvent { event: event_ptr }),
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
                        ValueKind::Disconnected => Some(Err(Disconnected)),
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

    fn sender_dropped(&self) {
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

/// Enables a sender or receiver to reference the event that connects them.
///
/// This is a sealed trait and exists for internal use only. You never need to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait EventRef<T>: Deref<Target = OnceEvent<T>> + Sealed
where
    T: Send,
{
}

/// An event referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Copy, Debug)]
pub struct ByRefEvent<'a, T>
where
    T: Send,
{
    event: &'a OnceEvent<T>,
}

impl<T> Sealed for ByRefEvent<'_, T> where T: Send {}
impl<T> EventRef<T> for ByRefEvent<'_, T> where T: Send {}
impl<T> Deref for ByRefEvent<'_, T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}
impl<T> Clone for ByRefEvent<'_, T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}

/// An event referenced via `Arc` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Debug)]
pub struct ByArcEvent<T>
where
    T: Send,
{
    event: Arc<OnceEvent<T>>,
}

impl<T> Sealed for ByArcEvent<T> where T: Send {}
impl<T> EventRef<T> for ByArcEvent<T> where T: Send {}
impl<T> Deref for ByArcEvent<T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}
impl<T> Clone for ByArcEvent<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self {
            event: Arc::clone(&self.event),
        }
    }
}

/// An event referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Copy, Debug)]
pub struct ByPtrEvent<T>
where
    T: Send,
{
    event: NonNull<OnceEvent<T>>,
}

impl<T> Sealed for ByPtrEvent<T> where T: Send {}
impl<T> EventRef<T> for ByPtrEvent<T> where T: Send {}
impl<T> Deref for ByPtrEvent<T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for ByPtrEvent<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
// SAFETY: This is only used with the thread-safe event (the event is Sync).
unsafe impl<T> Send for ByPtrEvent<T> where T: Send {}

/// A sender that can send a value through a single-threaded event using Rc ownership.
///
/// The sender owns an Rc to the event and is single-threaded.
/// After calling [`send`](OnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct OnceSender<T, R>
where
    T: Send,
    R: EventRef<T>,
{
    event_ref: R,

    _t: PhantomData<T>,

    // We do not expect use cases that require Sync, so we suppress it to leave
    // design flexibility for future changes.
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, R> OnceSender<T, R>
where
    T: Send,
    R: EventRef<T>,
{
    fn new(event_ref: R) -> Self {
        Self {
            event_ref,
            _t: PhantomData,
            _not_sync: PhantomData,
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
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let (sender, _receiver) = event.bind_by_arc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        self.event_ref.set(value);
    }
}

impl<T, R> Drop for OnceSender<T, R>
where
    T: Send,
    R: EventRef<T>,
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
pub struct OnceReceiver<T, R>
where
    T: Send,
    R: EventRef<T>,
{
    event_ref: R,

    _t: PhantomData<T>,

    // We do not expect use cases that require Sync, so we suppress it to leave
    // design flexibility for future changes.
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, R> OnceReceiver<T, R>
where
    T: Send,
    R: EventRef<T>,
{
    fn new(event_ref: R) -> Self {
        Self {
            event_ref,
            _t: PhantomData,
            _not_sync: PhantomData,
        }
    }
}

impl<T, R> Future for OnceReceiver<T, R>
where
    T: Send,
    R: EventRef<T>,
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
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::thread;

    use futures::task::noop_waker_ref;
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
    fn event_by_ptr_basic() {
        with_watchdog(|| {
            let event = Box::pin(OnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let event = Box::pin(OnceEvent::<String>::new());

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
                assert!(matches!(result, Err(Disconnected)));
            });
        });
    }

    #[test]
    fn sender_dropped_when_awaiting_signals_disconnected() {
        let event = OnceEvent::<i32>::new();
        let (sender, receiver) = event.bind_by_ref();

        let mut receiver = pin!(receiver);
        let mut context = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            receiver.as_mut().poll(&mut context),
            task::Poll::Pending
        ));

        drop(sender);

        let mut context = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            receiver.as_mut().poll(&mut context),
            task::Poll::Ready(Err(Disconnected))
        ));
    }

    #[test]
    fn thread_safety() {
        // The event is accessed across threads, so requires Sync as well as Send.
        assert_impl_all!(OnceEvent<u32>: Send, Sync);

        // These are all meant to be consumed ly - they may move between threads but are
        // not shared between threads, so Sync is not expected, only Send.
        assert_impl_all!(OnceSender<u32, ByRefEvent<'static, u32>>: Send);
        assert_impl_all!(OnceReceiver<u32, ByRefEvent<'static, u32>>: Send);
        assert_impl_all!(OnceSender<u32, ByArcEvent<u32>>: Send);
        assert_impl_all!(OnceReceiver<u32, ByArcEvent<u32>>: Send);
        assert_impl_all!(OnceSender<u32, ByPtrEvent<u32>>: Send);
        assert_impl_all!(OnceReceiver<u32, ByPtrEvent<u32>>: Send);
        assert_not_impl_any!(OnceSender<u32, ByRefEvent<'static, u32>>: Sync);
        assert_not_impl_any!(OnceReceiver<u32, ByRefEvent<'static, u32>>: Sync);
        assert_not_impl_any!(OnceSender<u32, ByArcEvent<u32>>: Sync);
        assert_not_impl_any!(OnceReceiver<u32, ByArcEvent<u32>>: Sync);
        assert_not_impl_any!(OnceSender<u32, ByPtrEvent<u32>>: Sync);
        assert_not_impl_any!(OnceReceiver<u32, ByPtrEvent<u32>>: Sync);
    }
}
